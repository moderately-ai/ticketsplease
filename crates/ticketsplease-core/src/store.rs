//! Ticket storage: load/save tickets, scaffold a repo, and generate ids.
//!
//! All writes are atomic (temp file + rename); new tickets are created with
//! `O_EXCL` so concurrent agents never clobber each other (R15).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rayon::prelude::*;

use crate::comment::Comment;
use crate::config::{Config, Recipe, CONFIG_FILE};
use crate::error::{Error, Result};
use crate::event::Event;
use crate::ids;
use crate::states::StateRegistry;
use crate::ticket::Ticket;

/// A repository handle: the root directory plus its loaded config.
pub struct Store {
    /// Repository root directory.
    pub repo_root: PathBuf,
    /// Loaded configuration.
    pub config: Config,
}

/// Outcome of creating a ticket with an explicit id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateOutcome {
    /// A new file was written.
    Created,
    /// An identical file already existed (idempotent no-op).
    Unchanged,
}

impl Store {
    /// Open a repository, loading its config (errors if not initialized).
    pub fn open(repo_root: &Path) -> Result<Self> {
        let config = Config::load(repo_root)?;
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            config,
        })
    }

    /// Every recipe available in this repo: the inline `[recipe.<name>]` tables merged
    /// with discovered `.ticketsplease/recipes/<name>.toml` files (filename = name,
    /// read in sorted order). A name defined in both places is a loud error.
    pub fn load_recipes(&self) -> Result<BTreeMap<String, Recipe>> {
        let mut out = self.config.recipes.clone();
        let dir = self.repo_root.join(".ticketsplease").join("recipes");
        if !dir.is_dir() {
            return Ok(out);
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(Error::Io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let raw = fs::read_to_string(&path).map_err(Error::Io)?;
            let recipe: Recipe = toml::from_str(&raw)
                .map_err(|e| Error::Invalid(format!("invalid recipe {}: {e}", path.display())))?;
            if out.contains_key(&name) {
                return Err(Error::Invalid(format!(
                    "recipe `{name}` is defined both inline in {CONFIG_FILE} and in {}",
                    path.display()
                )));
            }
            out.insert(name, recipe);
        }
        Ok(out)
    }

    /// Absolute path to the tickets directory.
    #[must_use]
    pub fn tickets_dir(&self) -> PathBuf {
        self.config.tickets_path(&self.repo_root)
    }

    /// Absolute path to a ticket file by id.
    #[must_use]
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.tickets_dir().join(format!("{id}.md"))
    }

    /// Sorted list of `*.md` ticket files (empty if the directory is absent).
    pub fn ticket_files(&self) -> Result<Vec<PathBuf>> {
        let dir = self.tickets_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(&dir)
            .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", dir.display())))?
        {
            let path = entry.map_err(Error::Io)?.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    /// Load and parse every ticket (sorted by id). Fails if any file is invalid.
    ///
    /// The per-file read + YAML parse is the dominant cost of every multi-ticket command,
    /// so the files are parsed in parallel across the machine's cores (see [`load_paths`]).
    pub fn load_all(&self) -> Result<Vec<Ticket>> {
        let reg = self.config.state_registry();
        let paths = self.ticket_files()?;
        let mut tickets = Vec::with_capacity(paths.len());
        for (path, res) in load_paths(&paths, &reg) {
            tickets.push(res.map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?);
        }
        tickets.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tickets)
    }

    /// Load every parseable ticket, returning warnings for files that failed to
    /// parse instead of aborting. Use for *display* commands (list/status) so one
    /// malformed file can't black out the whole board; scheduling commands keep
    /// the strict [`load_all`](Self::load_all). Parsed in parallel, like [`load_all`].
    pub fn load_all_lenient(&self) -> Result<(Vec<Ticket>, Vec<String>)> {
        let reg = self.config.state_registry();
        let paths = self.ticket_files()?;
        let mut tickets = Vec::with_capacity(paths.len());
        let mut warnings = Vec::new();
        for (path, res) in load_paths(&paths, &reg) {
            match res {
                Ok(t) => tickets.push(t),
                Err(e) => warnings.push(format!("{}: {}", path.display(), e.message())),
            }
        }
        tickets.sort_by(|a, b| a.id.cmp(&b.id));
        Ok((tickets, warnings))
    }

    /// Load a single ticket by id.
    pub fn load(&self, id: &str) -> Result<Ticket> {
        let path = self.path_for(id);
        if !path.exists() {
            return Err(Error::NotFound(id.to_string()));
        }
        let mut ticket = Ticket::load(&path)?;
        ticket.resolve_class(&self.config.state_registry());
        Ok(ticket)
    }

    /// Load a ticket as committed on a git ref (e.g. a `tkt/<id>` branch), via
    /// `git show <ref>:<tickets_dir>/<id>.md` — no checkout, no working-tree state.
    /// Lets an orchestrator on `main` observe a worker's in-flight status.
    pub fn load_at_ref(&self, id: &str, git_ref: &str) -> Result<Ticket> {
        let rel = format!("{}/{id}.md", self.config.tickets_dir);
        let spec = format!("{git_ref}:{rel}");
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["show", &spec])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let s = stderr.trim();
            // A missing ref or a path absent on that ref is "not found"; anything
            // else (e.g. not a git repo) is a usage/environment error.
            if s.contains("does not exist")
                || s.contains("exists on disk, but not in")
                || s.contains("unknown revision")
                || s.contains("invalid object name")
            {
                return Err(Error::NotFound(format!("{id} @ {git_ref}")));
            }
            return Err(Error::Invalid(format!("`git show {spec}` failed: {s}")));
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        let mut ticket =
            Ticket::parse(&raw).map_err(|e| Error::Invalid(format!("{id} @ {git_ref}: {e}")))?;
        ticket.resolve_class(&self.config.state_registry());
        Ok(ticket)
    }

    /// Load the config as committed on a git ref (e.g. the guard `--base`). Returns
    /// `Ok(None)` when the ref carries no config file, so a caller can fall back to
    /// the working-tree config. Lets the guard read the canonical `[scopes]` map from
    /// a stable ref instead of the possibly stale/empty config on a feature branch.
    pub fn config_at_ref(&self, git_ref: &str) -> Result<Option<Config>> {
        let spec = format!("{git_ref}:{CONFIG_FILE}");
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["show", &spec])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Config::parse(&text).map(Some)
    }

    /// Load the full ticket set with each `<prefix>*` branch tip overlaid onto its
    /// own ticket. In the branch-per-ticket flow a ticket's true in-flight status
    /// lives on its branch, so a plain working-tree load sees every sibling as
    /// whatever the *current* checkout says; this surfaces the real cross-branch
    /// status (the keystone for guard collision detection). Tickets without a branch
    /// keep their working-tree status. Returns lenient-load warnings alongside.
    pub fn load_all_cross_branch(&self, prefix: &str) -> Result<(Vec<Ticket>, Vec<String>)> {
        let (base, warnings) = self.load_all_lenient()?;
        let mut by_id: BTreeMap<String, Ticket> =
            base.into_iter().map(|t| (t.id.clone(), t)).collect();
        for (_branch, ticket) in self.load_branch_tickets(prefix)? {
            // A branch whose ticket file is absent on its tip (or unparseable) is simply
            // not overlaid; an overlaid ticket wins over its working-tree twin by id.
            if let Some(t) = ticket {
                by_id.insert(t.id.clone(), t);
            }
        }
        Ok((by_id.into_values().collect(), warnings))
    }

    /// Each `<prefix>*` branch paired with its ticket parsed from that branch's tip
    /// (`None` when the ticket file is absent on the tip or fails to parse), preserving
    /// the branch→ticket association the cross-branch overlay drops.
    ///
    /// Reads every branch's ticket file in a *single* `cat-file --batch` — 2 subprocesses
    /// total, not 1 + one `git show` per branch. This is the shared batching path behind
    /// both [`load_all_cross_branch`](Self::load_all_cross_branch) and the branch-source
    /// `status` scan; at thousands of `tkt/*` branches it is the guard/status hot path.
    pub fn load_branch_tickets(&self, prefix: &str) -> Result<Vec<(String, Option<Ticket>)>> {
        let branches = self.branches_with_prefix(prefix)?;
        let specs: Vec<String> = branches
            .iter()
            .map(|branch| {
                let id = branch.strip_prefix(prefix).unwrap_or(branch);
                format!("{branch}:{}/{id}.md", self.config.tickets_dir)
            })
            .collect();
        let reg = self.config.state_registry();
        let blobs = self.cat_file_batch(&specs)?;
        Ok(branches
            .into_iter()
            .zip(blobs)
            .map(|(branch, blob)| {
                let ticket = blob.and_then(|bytes| {
                    let raw = String::from_utf8_lossy(&bytes);
                    Ticket::parse(&raw).ok().map(|mut t| {
                        t.resolve_class(&reg);
                        t
                    })
                });
                (branch, ticket)
            })
            .collect())
    }

    /// Local branch names under `refs/heads/<prefix>*`. Empty (not an error) when
    /// there is no git repo or no matching branch.
    fn branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = format!("refs/heads/{prefix}*");
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["for-each-ref", "--format=%(refname:short)", &pattern])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            return Ok(Vec::new());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Directory holding a ticket's comment files.
    #[must_use]
    pub fn comments_dir(&self, id: &str) -> PathBuf {
        self.tickets_dir().join(format!("{id}.comments"))
    }

    /// Append a comment to a ticket — one file per comment, so concurrent authors
    /// never collide. The ticket must exist. Returns the created comment.
    pub fn add_comment(
        &self,
        ticket_id: &str,
        by: Option<String>,
        reply_to: Option<String>,
        body: &str,
    ) -> Result<Comment> {
        if !self.path_for(ticket_id).exists() {
            return Err(Error::NotFound(ticket_id.to_string()));
        }
        // A reply must target an existing comment, so a typo doesn't orphan it.
        if let Some(rt) = &reply_to {
            if !self.comments(ticket_id)?.iter().any(|c| &c.id == rt) {
                return Err(Error::NotFound(format!(
                    "comment `{rt}` to reply to on ticket `{ticket_id}`"
                )));
            }
        }
        let dir = self.comments_dir(ticket_id);
        fs::create_dir_all(&dir).map_err(Error::Io)?;
        let comment = Comment::new(by, reply_to, body);
        let path = dir.join(format!("{}.md", comment.id));
        // The id is unique, so create-new is just a belt-and-suspenders guard.
        create_exclusive(&path, &comment.render())?;
        Ok(comment)
    }

    /// All comments on a ticket from the working tree, sorted chronologically.
    pub fn comments(&self, ticket_id: &str) -> Result<Vec<Comment>> {
        let dir = self.comments_dir(ticket_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)
            .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", dir.display())))?
        {
            let path = entry.map_err(Error::Io)?.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                let raw = fs::read_to_string(&path).map_err(Error::Io)?;
                out.push(
                    Comment::parse(&raw)
                        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?,
                );
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Comments on a ticket as committed on a git ref (cross-branch read) — lists
    /// the comment tree on the ref and reads each blob. A missing comments tree on
    /// the ref means "no comments", not an error.
    pub fn comments_at_ref(&self, ticket_id: &str, git_ref: &str) -> Result<Vec<Comment>> {
        let rel = format!("{}/{ticket_id}.comments", self.config.tickets_dir);
        let tree = format!("{git_ref}:{rel}");
        let ls = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["ls-tree", "--name-only", &tree])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !ls.status.success() {
            return Ok(Vec::new());
        }
        // One `cat-file --batch` for every comment blob rather than a `git show` each.
        let listing = String::from_utf8_lossy(&ls.stdout);
        let specs: Vec<String> = listing
            .lines()
            .filter(|l| l.ends_with(".md"))
            .map(|name| format!("{git_ref}:{rel}/{name}"))
            .collect();
        let mut out = Vec::new();
        for blob in self.cat_file_batch(&specs)?.into_iter().flatten() {
            let raw = String::from_utf8_lossy(&blob);
            out.push(Comment::parse(&raw)?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Emit an activity event as a `refs/ticketsplease/events/<id>` ref pointing at
    /// a JSON blob. Lives entirely in `.git` (no working-tree change, no commit), so
    /// it is visible across worktrees and a shared clone immediately. Best-effort:
    /// returns `Ok(None)` when there is no git repo (the doorbell is auxiliary to
    /// the durable record). Concurrent emits never collide — the id is unique.
    pub fn emit_event(
        &self,
        kind: &str,
        ticket: &str,
        by: Option<&str>,
        data: serde_json::Value,
    ) -> Result<Option<Event>> {
        let event = Event {
            id: ids::new_id(),
            ticket: ticket.to_string(),
            kind: kind.to_string(),
            by: by.map(str::to_string),
            at: ids::now_secs(),
            data,
        };
        let payload = serde_json::to_string(&event)
            .map_err(|e| Error::Internal(format!("serializing event: {e}")))?;
        let blob = match self.git_hash_object(&payload)? {
            Some(sha) => sha,
            None => return Ok(None), // not a git repo — skip the doorbell
        };
        let refname = format!("refs/ticketsplease/events/{}", event.id);
        // Create-only (empty old-value): the id is unique, so this never clobbers.
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["update-ref", &refname, &blob, ""])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(Error::Invalid(format!(
                "git update-ref (event) failed: {}",
                err.trim()
            )));
        }
        Ok(Some(event))
    }

    /// All activity events, sorted chronologically by id. Empty when there is no
    /// git repo or no events yet.
    pub fn events(&self) -> Result<Vec<Event>> {
        self.events_since(None)
    }

    /// Activity events whose id sorts strictly after `since` (all of them when `None`),
    /// sorted chronologically by id. Empty when there is no git repo or no events yet.
    ///
    /// Two git processes total regardless of event count: one `for-each-ref` to list the
    /// event refs (as `<id> <blob-sha>` pairs), then a single `cat-file --batch` reading
    /// only the blobs newer than `since`. The `since` cursor is applied to the *refnames*
    /// before the batch, so a `--watch` poll dereferences only genuinely new events
    /// instead of re-reading the entire log every interval.
    pub fn events_since(&self, since: Option<&str>) -> Result<Vec<Event>> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args([
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/ticketsplease/events/",
            ])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !out.status.success() {
            return Ok(Vec::new());
        }
        let listing = String::from_utf8_lossy(&out.stdout);
        let mut shas = Vec::new();
        for line in listing.lines().filter(|l| !l.is_empty()) {
            // `refs/ticketsplease/events/<id> <sha>`
            let Some((refname, sha)) = line.rsplit_once(' ') else {
                continue;
            };
            let id = refname
                .strip_prefix("refs/ticketsplease/events/")
                .unwrap_or(refname);
            if since.map_or(true, |s| id > s) {
                shas.push(sha.to_string());
            }
        }
        let mut events: Vec<Event> = self
            .cat_file_batch(&shas)?
            .into_iter()
            .flatten()
            .filter_map(|bytes| serde_json::from_slice::<Event>(&bytes).ok())
            .collect();
        events.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(events)
    }

    /// Delete every event ref whose id sorts strictly before `before`, compacting the log,
    /// and return how many were pruned. One `for-each-ref` to list plus one
    /// `update-ref --stdin` to delete the whole batch — never a delete per ref.
    ///
    /// The event log is the live coordination doorbell, not the durable record (that is
    /// the tickets themselves), so dropping historical events is safe. Bounding the log
    /// this way is the counterpart to the batched read: reads are cheap per event now, but
    /// the ref count itself should still be boundable on a long-lived repo.
    pub fn prune_events_before(&self, before: &str) -> Result<usize> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args([
                "for-each-ref",
                "--format=%(refname)",
                "refs/ticketsplease/events/",
            ])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !out.status.success() {
            return Ok(0);
        }
        let listing = String::from_utf8_lossy(&out.stdout);
        let victims: Vec<&str> = listing
            .lines()
            .filter(|refname| {
                let id = refname
                    .strip_prefix("refs/ticketsplease/events/")
                    .unwrap_or(refname);
                id < before
            })
            .collect();
        if victims.is_empty() {
            return Ok(0);
        }
        // `update-ref --stdin` applies all deletions in one transaction. Omitting the
        // old-value means "delete whatever it points at" — correct for a GC sweep.
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Internal("git update-ref stdin unavailable".into()))?;
        let commands: String = victims.iter().map(|r| format!("delete {r}\n")).collect();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(commands.as_bytes());
        });
        let done = child.wait_with_output().map_err(Error::Io)?;
        let _ = writer.join();
        if !done.status.success() {
            let err = String::from_utf8_lossy(&done.stderr);
            return Err(Error::Invalid(format!(
                "git update-ref --stdin (prune) failed: {}",
                err.trim()
            )));
        }
        Ok(victims.len())
    }

    /// Write `content` to the object store as a loose blob, returning its sha.
    /// `Ok(None)` when this is not a git repo.
    fn git_hash_object(&self, content: &str) -> Result<Option<String>> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        child
            .stdin
            .take()
            .ok_or_else(|| Error::Internal("git hash-object stdin unavailable".into()))?
            .write_all(content.as_bytes())
            .map_err(Error::Io)?;
        let out = child.wait_with_output().map_err(Error::Io)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("not a git repository") {
                return Ok(None);
            }
            return Err(Error::Invalid(format!(
                "git hash-object failed: {}",
                err.trim()
            )));
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    /// Read many git objects in a *single* `git cat-file --batch` process, returning each
    /// spec's raw content in input order (`None` for a missing object). `specs` are
    /// anything cat-file accepts on stdin — object shas, refnames, or `<rev>:<path>`.
    ///
    /// This is the batching primitive that collapses the store's per-item `git show` /
    /// `git cat-file -p` fan-outs (events, cross-branch overlays, cross-ref comments) from
    /// one subprocess *per object* to one subprocess *total*. At thousands of events or
    /// branches the process-spawn cost dominated everything else; one pipe replaces it.
    ///
    /// stdin is fed from a separate thread so a large stdout can drain concurrently — the
    /// classic write-all-then-read pipe deadlock cannot occur.
    fn cat_file_batch(&self, specs: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if specs.is_empty() {
            return Ok(Vec::new());
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Internal("git cat-file stdin unavailable".into()))?;
        let input = specs.join("\n");
        let writer = std::thread::spawn(move || {
            // Best-effort: if git exits early (e.g. a bad spec) the write errors; the
            // caller surfaces that via the parsed output, not this thread.
            let _ = stdin.write_all(input.as_bytes());
            let _ = stdin.write_all(b"\n");
            // Dropping `stdin` here closes it, signalling EOF so git flushes and exits.
        });
        let out = child.wait_with_output().map_err(Error::Io)?;
        let _ = writer.join();
        if !out.status.success() {
            return Ok(specs.iter().map(|_| None).collect());
        }
        Ok(parse_cat_file_batch(&out.stdout, specs.len()))
    }

    /// Atomically overwrite a ticket file. Writes back to the path the ticket was
    /// loaded from when known, so an `id` that has drifted from its filename does
    /// not orphan the original file (or mint a duplicate id); falls back to
    /// `<id>.md` for tickets built in memory.
    pub fn save(&self, ticket: &Ticket) -> Result<()> {
        let path = ticket
            .source_path()
            .map_or_else(|| self.path_for(&ticket.id), Path::to_path_buf);
        write_atomic(&path, &ticket.render())
    }

    /// Create a ticket with an explicit id (idempotent + atomic). Re-creating
    /// with byte-identical content is a no-op; differing content is an error.
    pub fn create_exact(&self, id: &str, contents: &str) -> Result<CreateOutcome> {
        validate_slug(id)?;
        let path = self.path_for(id);
        match create_exclusive(&path, contents) {
            Ok(()) => Ok(CreateOutcome::Created),
            Err(Error::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(&path).map_err(Error::Io)?;
                if existing == contents {
                    Ok(CreateOutcome::Unchanged)
                } else {
                    Err(Error::Invalid(format!(
                        "ticket `{id}` already exists with different content"
                    )))
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Create a ticket choosing a unique id from `base_id` (`-2`, `-3`, ... on
    /// collision). `render` builds the file contents for the chosen id. Atomic.
    pub fn create_unique(
        &self,
        base_id: &str,
        render: impl Fn(&str) -> Result<String>,
    ) -> Result<String> {
        self.create_unique_idempotent(base_id, render)
            .map(|(id, _)| id)
    }

    /// Like [`Self::create_unique`], but idempotent by content: if an existing
    /// `<base_id>`/`<base_id>-N` already holds byte-identical content, return it
    /// `Unchanged` instead of minting a duplicate — so re-running the same auto-id
    /// create (or batch) is a no-op rather than a clone. A differing ticket at a
    /// candidate id is skipped to the next suffix, as before.
    pub fn create_unique_idempotent(
        &self,
        base_id: &str,
        render: impl Fn(&str) -> Result<String>,
    ) -> Result<(String, CreateOutcome)> {
        for n in 1u32.. {
            let id = if n == 1 {
                base_id.to_string()
            } else {
                format!("{base_id}-{n}")
            };
            let path = self.path_for(&id);
            let contents = render(&id)?;
            match create_exclusive(&path, &contents) {
                Ok(()) => return Ok((id, CreateOutcome::Created)),
                Err(Error::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists => {
                    // Same content at this id -> it's the same ticket (idempotent);
                    // different content -> a distinct ticket, try the next suffix.
                    let existing = fs::read_to_string(&path).map_err(Error::Io)?;
                    if existing == contents {
                        return Ok((id, CreateOutcome::Unchanged));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!("u32 id-suffix range is effectively unbounded")
    }
}

/// Outcome of [`init_repo`].
pub struct InitOutcome {
    /// The tickets directory that now exists.
    pub tickets_dir: PathBuf,
    /// Whether a fresh config file was written.
    pub wrote_config: bool,
}

/// Scaffold a repository: create the tickets directory and (unless one exists) a
/// templated `ticketsplease.toml`. Idempotent unless `force`.
pub fn init_repo(
    repo_root: &Path,
    tickets_dir: &str,
    config_body: &str,
    force: bool,
) -> Result<InitOutcome> {
    let dir = repo_root.join(tickets_dir);
    fs::create_dir_all(&dir).map_err(Error::Io)?;
    let config_path = repo_root.join(CONFIG_FILE);
    let wrote_config = if force || !config_path.exists() {
        write_atomic(&config_path, config_body)?;
        true
    } else {
        false
    };
    Ok(InitOutcome {
        tickets_dir: dir,
        wrote_config,
    })
}

/// Validate a ticket id is a safe slug: lowercase ASCII alphanumerics joined by
/// single hyphens (no leading/trailing/double hyphen, no path separators). This is
/// the gate that stops an explicit `--id` from escaping the tickets directory
/// (`../x`), crashing on a separator, or producing a non-portable filename.
pub fn validate_slug(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "invalid ticket id `{id}` (use lowercase letters, digits, and single hyphens)"
        )))
    }
}

/// Derive a slug id from a title: lowercase ASCII alphanumerics, with other runs
/// collapsed to single `-`. Empty results fall back to `ticket`.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "ticket".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The default `ticketsplease.toml` body (path-glob backend, commented examples).
#[must_use]
pub fn default_config_template(tickets_dir: &str) -> String {
    format!(
        "schema_version = 1\n\
         tickets_dir = \"{tickets_dir}\"\n\
         default_base = \"main\"\n\
         \n\
         [guard]\n\
         # A declared-area overlap with an open sibling is a non-failing WARN by default\n\
         # (exit 0); an under-declaration (scope escape) always fails (exit 6). Set true\n\
         # (or pass `guard --strict`) to make an overlap gate too.\n\
         # gate_collisions = false\n\
         \n\
         [language]\n\
         # \"none\" = path-glob scopes only; \"rust\" = also expand via the cargo crate graph.\n\
         backend = \"none\"\n\
         \n\
         # Map abstract scope names to path globs. Tickets reference these stable names.\n\
         [scopes]\n\
         # \"query/planner\" = [\"crates/query/src/planner/**\"]\n\
         \n\
         # Optionally map a scope to its owning crate so the Rust backend can expand\n\
         # reverse-dependents (requires `cargo` at runtime).\n\
         [scope_crates]\n\
         # \"core\" = \"my-core-crate\"\n\
         \n\
         # Name a forked/external dependency (pinned via `git = … rev = …`) as a scope.\n\
         # The guard flags a branch that bumps the pin (matched by `repo`) or edits an\n\
         # in-tree fork `paths` glob, against tickets declaring the same scope.\n\
         [external_scopes]\n\
         # \"sqlparser-fork\" = {{ repo = \"tomsanbear/sqlparser\", paths = [] }}\n\
         \n\
         # Tune how costly an exclusive overlap on a scope is for tracks/next\n\
         # (--max-overlap): weight 0 = free to co-edit (an additive hub), higher =\n\
         # riskier. Default 1; a shared-by-both claim is always free.\n\
         [scope_policy]\n\
         # \"core\" = {{ weight = 0 }}\n"
    )
}

/// Parse the output of `git cat-file --batch` into one entry per requested spec.
///
/// The `--batch` record for a found object is `<oid> <type> <size>\n<content>\n`; for a
/// missing one it is `<spec> missing\n`. Content is read by its declared byte length (it
/// may contain newlines), never by line-splitting. Malformed or truncated output stops
/// parsing and pads the remainder with `None` rather than panicking.
fn parse_cat_file_batch(data: &[u8], expected: usize) -> Vec<Option<Vec<u8>>> {
    let mut results = Vec::with_capacity(expected);
    let mut i = 0;
    while results.len() < expected {
        let Some(rel_nl) = data
            .get(i..)
            .and_then(|s| s.iter().position(|&b| b == b'\n'))
        else {
            break;
        };
        let header = &data[i..i + rel_nl];
        i += rel_nl + 1;
        if header.ends_with(b" missing") {
            results.push(None);
            continue;
        }
        // `<oid> <type> <size>` — size is the last space-separated token.
        let size = std::str::from_utf8(header)
            .ok()
            .and_then(|h| h.rsplit(' ').next())
            .and_then(|s| s.parse::<usize>().ok());
        let Some(size) = size else { break };
        if i + size > data.len() {
            break; // truncated content — treat the rest as unavailable
        }
        results.push(Some(data[i..i + size].to_vec()));
        i += size + 1; // skip the content and its trailing newline
    }
    while results.len() < expected {
        results.push(None);
    }
    results
}

/// Load, parse, and class-resolve each ticket file in parallel across the machine's
/// cores, preserving input order. Each result is paired with its path so a caller can
/// attribute a strict error or a lenient warning. Rayon's work-stealing balances the
/// uneven per-file cost (large bodies, long dependency lists) better than a fixed split.
fn load_paths(paths: &[PathBuf], reg: &StateRegistry) -> Vec<(PathBuf, Result<Ticket>)> {
    paths
        .par_iter()
        .map(|path| {
            let ticket = Ticket::load(path).map(|mut t| {
                t.resolve_class(reg);
                t
            });
            (path.clone(), ticket)
        })
        .collect()
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("ticket.md");
    let tmp = dir.join(format!(".{file_name}.tmp.{}", std::process::id()));
    {
        let mut f = File::create(&tmp).map_err(Error::Io)?;
        f.write_all(contents.as_bytes()).map_err(Error::Io)?;
        f.sync_all().map_err(Error::Io)?;
    }
    fs::rename(&tmp, path).map_err(Error::Io)?;
    Ok(())
}

fn create_exclusive(path: &Path, contents: &str) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(Error::Io)?;
    f.write_all(contents.as_bytes()).map_err(Error::Io)?;
    f.sync_all().map_err(Error::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cat_file_batch_parses_mixed_present_missing_and_binary_content() {
        // Two found objects (the second's content contains a newline, so it must be read
        // by declared byte length, not line-split) with a missing one between them.
        let body_a = "first";
        let body_b = "line1\nline2"; // 11 bytes, embedded newline
        let mut data = Vec::new();
        data.extend_from_slice(format!("aaaa blob {}\n", body_a.len()).as_bytes());
        data.extend_from_slice(body_a.as_bytes());
        data.push(b'\n');
        data.extend_from_slice(b"deadbeef missing\n");
        data.extend_from_slice(format!("bbbb blob {}\n", body_b.len()).as_bytes());
        data.extend_from_slice(body_b.as_bytes());
        data.push(b'\n');

        let out = parse_cat_file_batch(&data, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].as_deref(), Some(body_a.as_bytes()));
        assert_eq!(out[1], None, "the missing object maps to None");
        assert_eq!(out[2].as_deref(), Some(body_b.as_bytes()));
    }

    #[test]
    fn cat_file_batch_pads_truncated_output_with_none() {
        // Only one record for two expected specs -> the second is padded None, no panic.
        let data = b"aaaa blob 2\nhi\n";
        let out = parse_cat_file_batch(data, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].as_deref(), Some(&b"hi"[..]));
        assert_eq!(out[1], None);
        // Empty input for a non-zero expectation is all None, not a panic.
        assert_eq!(parse_cat_file_batch(b"", 2), vec![None, None]);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Add Vector Index"), "add-vector-index");
        assert_eq!(slugify("  Hello,  World!! "), "hello-world");
        assert_eq!(slugify("***"), "ticket");
        assert_eq!(slugify("Already-slug"), "already-slug");
    }

    #[test]
    fn validate_slug_accepts_good_rejects_bad() {
        for ok in ["a", "a1", "ux-sanitize-ticket-id", "build-index-2"] {
            assert!(validate_slug(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "../x", "a/b", "UPPER", "a b", "a--b", "-x", "x-", "", "Add", "a.b",
        ] {
            assert!(validate_slug(bad).is_err(), "{bad} should be rejected");
        }
    }
}
