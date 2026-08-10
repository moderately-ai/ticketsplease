//! Ticket storage: load/save tickets, scaffold a repo, and generate ids.
//!
//! All writes are atomic (temp file + rename); new tickets are created with
//! `O_EXCL` so concurrent agents never clobber each other (R15).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rayon::prelude::*;

use crate::comment::{Comment, CommentQuery, CommentSummary, CommentThread, SourcedComment};
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

/// A ticket and its complete, source-aware comment thread.
#[derive(Debug, Clone)]
pub struct TicketDetails {
    pub ticket: Ticket,
    pub comments: CommentThread,
}

#[derive(Debug, Clone)]
enum CommentLocation {
    Worktree(PathBuf),
    GitBlob(String),
}

#[derive(Debug, Clone)]
struct InventoryEntry {
    source: String,
    location: CommentLocation,
}

type CommentInventory = BTreeMap<String, BTreeMap<String, Vec<InventoryEntry>>>;

impl Store {
    /// Open a repository, loading its config (errors if not initialized).
    pub fn open(repo_root: &Path) -> Result<Self> {
        let config = Config::load(repo_root)?;
        let store = Self {
            repo_root: repo_root.to_path_buf(),
            config,
        };
        // Finish or roll back any incomplete multi-file transaction left by a crash.
        store.recover_pending_txn()?;
        Ok(store)
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
        // A reply must target an existing comment, so a typo doesn't orphan it. A comment
        // id equals its file stem, so check that file's existence directly rather than
        // reading and parsing the whole thread. Reject any id bearing a path separator so
        // a crafted `--reply-to` cannot escape the comments directory.
        if let Some(rt) = &reply_to {
            let safe =
                !rt.is_empty() && !rt.contains('/') && !rt.contains('\\') && !rt.contains("..");
            if !safe
                || !self
                    .comments_dir(ticket_id)
                    .join(format!("{rt}.md"))
                    .exists()
            {
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

    /// Count comments for many tickets without parsing their bodies. Worktree comment
    /// directories are scanned once; git comment trees are read through one batch.
    pub fn comment_summaries(
        &self,
        ticket_ids: &[String],
        query: &CommentQuery,
    ) -> Result<BTreeMap<String, CommentSummary>> {
        let inventory = self.comment_inventory(ticket_ids, query)?;
        Ok(ticket_ids
            .iter()
            .map(|id| {
                let entries = inventory.get(id);
                let mut sources = BTreeMap::new();
                if let Some(entries) = entries {
                    for locations in entries.values() {
                        for location in locations {
                            *sources.entry(location.source.clone()).or_insert(0) += 1;
                        }
                    }
                }
                (
                    id.clone(),
                    CommentSummary {
                        count: entries.map_or(0, BTreeMap::len),
                        sources,
                    },
                )
            })
            .collect())
    }

    /// Load and deduplicate complete comment threads for many tickets. Git blobs for
    /// every selected ticket are fetched in one `cat-file --batch` invocation.
    pub fn comment_threads(
        &self,
        ticket_ids: &[String],
        query: &CommentQuery,
    ) -> Result<BTreeMap<String, CommentThread>> {
        let inventory = self.comment_inventory(ticket_ids, query)?;
        let blob_specs: Vec<String> = inventory
            .values()
            .flat_map(BTreeMap::values)
            .flatten()
            .filter_map(|e| match &e.location {
                CommentLocation::GitBlob(oid) => Some(oid.clone()),
                CommentLocation::Worktree(_) => None,
            })
            .collect();
        let blobs = self.cat_file_batch(&blob_specs)?;
        let blobs_by_oid: BTreeMap<String, Option<Vec<u8>>> =
            blob_specs.into_iter().zip(blobs).collect();
        let mut threads = BTreeMap::new();

        for id in ticket_ids {
            let Some(by_comment) = inventory.get(id) else {
                threads.insert(id.clone(), CommentThread::default());
                continue;
            };
            let mut sources = BTreeMap::new();
            let mut comments = Vec::with_capacity(by_comment.len());
            for (comment_id, locations) in by_comment {
                let mut parsed: Option<Comment> = None;
                let mut origins = Vec::with_capacity(locations.len());
                for location in locations {
                    *sources.entry(location.source.clone()).or_insert(0) += 1;
                    origins.push(location.source.clone());
                    let raw = match &location.location {
                        CommentLocation::Worktree(path) => {
                            fs::read_to_string(path).map_err(Error::Io)?
                        }
                        CommentLocation::GitBlob(oid) => {
                            let bytes = blobs_by_oid.get(oid).cloned().flatten().ok_or_else(|| {
                                Error::Invalid(format!(
                                    "comment `{comment_id}` on ticket `{id}` disappeared while reading git"
                                ))
                            })?;
                            String::from_utf8(bytes).map_err(|e| {
                                Error::Invalid(format!(
                                    "comment `{comment_id}` on ticket `{id}` is not UTF-8: {e}"
                                ))
                            })?
                        }
                    };
                    let comment = Comment::parse(&raw).map_err(|e| {
                        Error::Invalid(format!("comment `{comment_id}` on ticket `{id}`: {e}"))
                    })?;
                    if comment.id != *comment_id {
                        return Err(Error::Invalid(format!(
                            "comment file `{comment_id}.md` on ticket `{id}` declares id `{}`",
                            comment.id
                        )));
                    }
                    if let Some(existing) = &parsed {
                        if existing != &comment {
                            return Err(Error::Invalid(format!(
                                "comment `{comment_id}` on ticket `{id}` differs across sources"
                            )));
                        }
                    } else {
                        parsed = Some(comment);
                    }
                }
                origins.sort();
                comments.push(SourcedComment {
                    comment: parsed.expect("inventory entries are never empty"),
                    sources: origins,
                });
            }
            threads.insert(
                id.clone(),
                CommentThread {
                    summary: CommentSummary {
                        count: by_comment.len(),
                        sources,
                    },
                    comments,
                },
            );
        }
        Ok(threads)
    }

    /// Load one ticket and its complete comment thread. An exact ref supplies both the
    /// ticket and comments; other queries keep the ticket in the current worktree.
    pub fn load_details(&self, id: &str, query: &CommentQuery) -> Result<TicketDetails> {
        let ticket = match query {
            CommentQuery::Ref { git_ref } => self.load_at_ref(id, git_ref)?,
            _ => self.load(id)?,
        };
        let mut threads = self.comment_threads(&[id.to_string()], query)?;
        Ok(TicketDetails {
            ticket,
            comments: threads.remove(id).unwrap_or_default(),
        })
    }

    fn comment_inventory(
        &self,
        ticket_ids: &[String],
        query: &CommentQuery,
    ) -> Result<CommentInventory> {
        let wanted: BTreeSet<&str> = ticket_ids.iter().map(String::as_str).collect();
        let mut inventory = CommentInventory::new();
        if matches!(query, CommentQuery::Worktree | CommentQuery::All { .. }) {
            let tickets_dir = self.tickets_dir();
            if tickets_dir.is_dir() {
                for entry in fs::read_dir(&tickets_dir).map_err(Error::Io)? {
                    let entry = entry.map_err(Error::Io)?;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let Some(id) = name.strip_suffix(".comments") else {
                        continue;
                    };
                    if !wanted.contains(id) {
                        continue;
                    }
                    for file in fs::read_dir(&path).map_err(Error::Io)? {
                        let file = file.map_err(Error::Io)?.path();
                        if file.extension().is_some_and(|ext| ext == "md") {
                            if let Some(comment_id) = file.file_stem().and_then(|s| s.to_str()) {
                                inventory
                                    .entry(id.to_string())
                                    .or_default()
                                    .entry(comment_id.to_string())
                                    .or_default()
                                    .push(InventoryEntry {
                                        source: "worktree".to_string(),
                                        location: CommentLocation::Worktree(file),
                                    });
                            }
                        }
                    }
                }
            }
        }

        let refs: Vec<(String, String)> = match query {
            CommentQuery::Worktree => Vec::new(),
            CommentQuery::TicketBranch { prefix } | CommentQuery::All { prefix } => ticket_ids
                .iter()
                .map(|id| (id.clone(), format!("{prefix}{id}")))
                .collect(),
            CommentQuery::Ref { git_ref } => ticket_ids
                .iter()
                .map(|id| (id.clone(), git_ref.clone()))
                .collect(),
        };
        if refs.is_empty() {
            return Ok(inventory);
        }

        let oid_len = match self.git_oid_len() {
            Ok(len) => len,
            Err(_) if matches!(query, CommentQuery::All { .. }) => return Ok(inventory),
            Err(e) => return Err(e),
        };
        let specs: Vec<String> = refs
            .iter()
            .map(|(id, git_ref)| format!("{git_ref}:{}/{id}.comments", self.config.tickets_dir))
            .collect();
        let trees = self.cat_file_batch(&specs)?;
        for ((id, git_ref), tree) in refs.into_iter().zip(trees) {
            let Some(tree) = tree else { continue };
            for (name, oid) in parse_git_tree(&tree, oid_len)? {
                let Some(comment_id) = name.strip_suffix(".md") else {
                    continue;
                };
                inventory
                    .entry(id.clone())
                    .or_default()
                    .entry(comment_id.to_string())
                    .or_default()
                    .push(InventoryEntry {
                        source: git_ref.clone(),
                        location: CommentLocation::GitBlob(oid),
                    });
            }
        }
        Ok(inventory)
    }

    fn git_oid_len(&self) -> Result<usize> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["rev-parse", "--show-object-format"])
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            return Err(Error::Invalid(
                "comment source requires a git repository".into(),
            ));
        }
        match String::from_utf8_lossy(&output.stdout).trim() {
            "sha1" => Ok(20),
            "sha256" => Ok(32),
            other => Err(Error::Invalid(format!(
                "unsupported git object format `{other}`"
            ))),
        }
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

    /// Snapshot for pure planning: lenient tickets + raw file contents for
    /// content-identical Unchanged checks. Malformed files are omitted from
    /// `tickets` but still appear in `contents_by_id` when readable.
    pub fn snapshot_for_plan(&self) -> Result<crate::plan::BoardSnapshot> {
        let (tickets, _warnings) = self.load_all_lenient()?;
        let mut contents_by_id = BTreeMap::new();
        for path in self.ticket_files()? {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Ok(raw) = fs::read_to_string(&path) {
                contents_by_id.insert(stem.to_string(), raw);
            }
        }
        Ok(crate::plan::BoardSnapshot::with_contents(
            tickets,
            contents_by_id,
        ))
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
         # Defaults are written into ticket frontmatter; explicit exclusive scopes win.\n\
         [defaults]\n\
         # shared_scopes = [\"project/tickets\"]\n\
         \n\
         [output]\n\
         # Detail commands show full threads; collections show counts. Comment reads\n\
         # union this worktree with the matching tkt/<id> branch by default.\n\
         comments = \"auto\"\n\
         comment_source = \"all\"\n\
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

/// Parse the raw contents of a git tree object: `<mode> <name>\0<raw oid>` entries.
/// Comment directories are flat, so only blob names and ids are needed.
fn parse_git_tree(data: &[u8], oid_len: usize) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let Some(space) = data[i..].iter().position(|&b| b == b' ') else {
            return Err(Error::Invalid("malformed git comment tree".into()));
        };
        i += space + 1;
        let Some(nul) = data[i..].iter().position(|&b| b == 0) else {
            return Err(Error::Invalid("malformed git comment tree".into()));
        };
        let name = std::str::from_utf8(&data[i..i + nul])
            .map_err(|e| Error::Invalid(format!("non-UTF-8 comment filename in git: {e}")))?
            .to_string();
        i += nul + 1;
        if i + oid_len > data.len() {
            return Err(Error::Invalid("truncated git comment tree".into()));
        }
        let oid = data[i..i + oid_len]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        i += oid_len;
        out.push((name, oid));
    }
    Ok(out)
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

/// Create a new file exclusively (O_EXCL). Used by single-ticket create and by
/// journaled multi-create publish. `pub(crate)` so `txn` can publish staged bodies.
pub(crate) fn create_exclusive(path: &Path, contents: &str) -> Result<()> {
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
    fn parses_raw_git_tree_entries() {
        let mut tree = b"100644 a.md\0".to_vec();
        tree.extend([0xabu8; 20]);
        tree.extend_from_slice(b"100644 b.md\0");
        tree.extend([0xcdu8; 20]);
        let entries = parse_git_tree(&tree, 20).unwrap();
        assert_eq!(entries[0], ("a.md".into(), "ab".repeat(20)));
        assert_eq!(entries[1], ("b.md".into(), "cd".repeat(20)));
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
