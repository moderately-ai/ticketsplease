//! Command handlers. Each emits human-readable text by default and a stable,
//! versioned JSON payload under `--format json`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use ticketsplease_cargo::{workspace_members, CargoMapper, WorkspaceMember};
use ticketsplease_core::claim as claim_core;
use ticketsplease_core::comment::Comment;
use ticketsplease_core::config::{Backend, CONFIG_FILE};
use ticketsplease_core::event::Event;
use ticketsplease_core::guard;
use ticketsplease_core::migrate as migrate_core;
use ticketsplease_core::store::{self, CreateOutcome};
use ticketsplease_core::{
    lint as lint_core, schedule, Error, Priority, Result, Status, Store, Ticket,
};

use crate::cli::{
    ClaimArgs, ClaimsArgs, CommentAddArgs, CommentListArgs, CreateArgs, EventsArgs, GuardArgs,
    InitArgs, LinkArgs, ListArgs, NextArgs, ReleaseArgs, SelfUpdateArgs, SetArgs, ShowArgs,
    SkillInstallArgs, StatusArgs, WatchArgs, WhyArgs,
};
use crate::format::{print_json, Format};
use crate::skill;
use crate::update;

/// `init` — scaffold the tickets directory and config.
pub fn init(repo: &Path, fmt: Format, args: &InitArgs) -> Result<()> {
    let config_body = build_config(repo, &args.dir);
    let outcome = store::init_repo(repo, &args.dir, &config_body, args.force)?;
    let dir = outcome.tickets_dir.display().to_string();
    let skill_path = if args.no_skill {
        None
    } else {
        Some(
            skill::install(repo, ".claude/skills")?
                .display()
                .to_string(),
        )
    };
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "tickets_dir": dir,
            "wrote_config": outcome.wrote_config,
            "skill_installed": skill_path,
        })),
        Format::Human => {
            println!("Initialized ticketsplease (tickets dir: {dir})");
            if !outcome.wrote_config {
                println!("(config already present; left unchanged)");
            }
            if let Some(path) = &skill_path {
                println!("Installed Claude skill to {path}");
            }
            Ok(())
        }
    }
}

/// `skill install` — write the bundled Claude skill into the repo.
pub fn skill_install(repo: &Path, fmt: Format, args: &SkillInstallArgs) -> Result<()> {
    let target = skill::install(repo, &args.dir)?;
    let path = target.display().to_string();
    match fmt {
        Format::Json => print_json(&json!({ "schema_version": 1, "installed": path })),
        Format::Human => {
            println!("Installed skill to {path}");
            Ok(())
        }
    }
}

/// `self-update` — replace the binary in place from GitHub Releases.
pub fn self_update(fmt: Format, args: &SelfUpdateArgs) -> Result<()> {
    update::run(args.version.as_deref())?;
    match fmt {
        Format::Json => print_json(&json!({ "schema_version": 1, "updated": true })),
        Format::Human => {
            println!("Updated ticketsplease via the installer");
            Ok(())
        }
    }
}

/// `create` — write a new ticket (idempotent with an explicit `--id`).
pub fn create(repo: &Path, fmt: Format, args: &CreateArgs) -> Result<()> {
    let store = Store::open(repo)?;
    if let Some(from) = &args.from {
        return create_batch(&store, fmt, from);
    }
    let title = args
        .title
        .as_deref()
        .ok_or_else(|| Error::Invalid("provide --title or --from".into()))?;
    let status: Status = args.status.parse()?;
    let priority: Priority = args.priority.parse()?;
    let depends_on = norm_list(&args.depends_on);
    let scopes = norm_list(&args.scopes);
    let paths = norm_list(&args.paths);
    let tags = norm_list(&args.tags);

    let build = |id: &str| -> Result<String> {
        Ticket::new(
            id,
            title,
            status,
            priority,
            &depends_on,
            &scopes,
            &paths,
            &tags,
            &args.body,
        )
        .map(|t| t.render())
    };

    let (id, outcome) = if let Some(id) = &args.id {
        let contents = build(id)?;
        (id.clone(), store.create_exact(id, &contents)?)
    } else {
        let base = store::slugify(title);
        (store.create_unique(&base, build)?, CreateOutcome::Created)
    };

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": id,
            "created": outcome == CreateOutcome::Created,
            "path": store.path_for(&id).display().to_string(),
        })),
        Format::Human => {
            match outcome {
                CreateOutcome::Created => println!("Created ticket `{id}`"),
                CreateOutcome::Unchanged => println!("Ticket `{id}` already exists (unchanged)"),
            }
            Ok(())
        }
    }
}

/// One element of a `create --from` batch.
#[derive(Deserialize)]
struct TicketSpec {
    title: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default, alias = "dependencies")]
    depends_on: Vec<String>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    body: String,
}

/// Batch-create from a JSON array of specs (file path, or `-` for stdin). Each
/// ticket is written atomically; an explicit `id` makes that element idempotent.
fn create_batch(store: &Store, fmt: Format, from: &str) -> Result<()> {
    let raw = read_text(from)?;
    let specs: Vec<TicketSpec> = serde_json::from_str(&raw).map_err(|e| {
        Error::Invalid(format!(
            "invalid --from JSON (expected an array of ticket specs): {e}"
        ))
    })?;

    let mut created = Vec::new();
    for spec in &specs {
        let status: Status = spec.status.as_deref().unwrap_or("todo").parse()?;
        let priority: Priority = spec.priority.as_deref().unwrap_or("p2").parse()?;
        let depends_on = norm_list(&spec.depends_on);
        let scopes = norm_list(&spec.scopes);
        let paths = norm_list(&spec.paths);
        let tags = norm_list(&spec.tags);
        let build = |id: &str| -> Result<String> {
            Ticket::new(
                id,
                &spec.title,
                status,
                priority,
                &depends_on,
                &scopes,
                &paths,
                &tags,
                &spec.body,
            )
            .map(|t| t.render())
        };
        let id = if let Some(id) = &spec.id {
            store.create_exact(id, &build(id)?)?;
            id.clone()
        } else {
            store.create_unique(&store::slugify(&spec.title), build)?
        };
        created.push(id);
    }

    match fmt {
        Format::Json => print_json(&json!({ "schema_version": 1, "created": created })),
        Format::Human => {
            println!(
                "Created {} ticket(s): {}",
                created.len(),
                created.join(", ")
            );
            Ok(())
        }
    }
}

/// `set` — surgically update a ticket's fields.
pub fn set(repo: &Path, fmt: Format, args: &SetArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let mut ticket = store.load(&args.id)?;
    let before = ticket.render();
    let status_before = ticket.status;

    if let Some(status) = &args.status {
        ticket.set_status(status.parse()?)?;
    }
    if let Some(priority) = &args.priority {
        ticket.set_priority(priority.parse()?)?;
    }
    for scope in norm_list(&args.add_scope) {
        ticket.add_scope(&scope)?;
    }
    for scope in norm_list(&args.remove_scope) {
        ticket.remove_scope(&scope)?;
    }
    for tag in norm_list(&args.add_tag) {
        ticket.add_tag(&tag)?;
    }
    for tag in norm_list(&args.remove_tag) {
        ticket.remove_tag(&tag)?;
    }
    if let Some(body) = body_input(args.body.as_deref(), args.body_file.as_deref())? {
        ticket.set_body(&body);
    }
    if let Some(text) = body_input(
        args.append_body.as_deref(),
        args.append_body_file.as_deref(),
    )? {
        ticket.append_body(&text);
    }
    // Completion implicitly ends a claim — a done ticket must not keep looking owned.
    if ticket.status == Status::Done {
        ticket.clear_lease();
    }

    let changed = ticket.render() != before;
    if changed {
        store.save(&ticket)?;
    }
    // A status transition is an activity event watchers care about.
    if changed && ticket.status != status_before {
        let _ = store.emit_event(
            "status",
            &ticket.id,
            None,
            json!({ "status": ticket.status.as_str(), "from": status_before.as_str() }),
        );
    }

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": ticket.id,
            "changed": changed,
        })),
        Format::Human => {
            println!(
                "{} `{}`",
                if changed { "Updated" } else { "No change to" },
                ticket.id
            );
            Ok(())
        }
    }
}

/// `link` — add or remove a dependency edge.
pub fn link(repo: &Path, fmt: Format, args: &LinkArgs) -> Result<()> {
    if args.id == args.depends_on {
        return Err(Error::Invalid("a ticket cannot depend on itself".into()));
    }
    let store = Store::open(repo)?;
    let mut ticket = store.load(&args.id)?;

    let changed = if args.remove {
        // Removal never validates the target: a dangling reference (its ticket was
        // deleted) must be cleanable without hand-editing the file.
        ticket.remove_dependency(&args.depends_on)?
    } else {
        ticket.add_dependency(&args.depends_on)?
    };

    // Adding an edge that closes a dependency cycle is rejected here (exit 5) rather
    // than left to corrupt the graph until `ready`/`tracks`/`next` trips over it. A
    // dangling target is permitted, mirroring `create --depends-on` — `lint` reports
    // both dangling deps and cycles on the parseable set.
    if changed && !args.remove {
        let mut all = store.load_all()?;
        if let Some(slot) = all.iter_mut().find(|t| t.id == ticket.id) {
            *slot = ticket.clone();
        }
        schedule::ensure_acyclic(&all)?;
    }

    if changed {
        store.save(&ticket)?;
    }

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": ticket.id,
            "depends_on": args.depends_on,
            "removed": args.remove,
            "changed": changed,
        })),
        Format::Human => {
            let verb = if args.remove { "Unlinked" } else { "Linked" };
            let note = if changed { "" } else { " (no change)" };
            println!("{verb} `{}` -> `{}`{note}", ticket.id, args.depends_on);
            Ok(())
        }
    }
}

/// `show` — print a single ticket and its comments, from the working tree or a
/// git ref (`--ref`).
pub fn show(repo: &Path, fmt: Format, args: &ShowArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let (ticket, comments) = match &args.r#ref {
        Some(git_ref) => (
            store.load_at_ref(&args.id, git_ref)?,
            store.comments_at_ref(&args.id, git_ref)?,
        ),
        None => (store.load(&args.id)?, store.comments(&args.id)?),
    };
    match fmt {
        Format::Json => {
            let mut v = ticket_json(&ticket);
            v["comments"] = json!(comments.iter().map(comment_value).collect::<Vec<_>>());
            print_json(&v)
        }
        Format::Human => {
            println!("{}  {}", ticket.id, ticket.title);
            println!("  status:   {}", ticket.status);
            println!("  priority: {}", ticket.priority);
            let line = |label: &str, items: &[String]| {
                if !items.is_empty() {
                    println!("  {label}: {}", items.join(", "));
                }
            };
            line("deps:    ", &ticket.dependencies);
            line("scopes:  ", &ticket.scopes);
            line("paths:   ", &ticket.paths);
            line("tags:    ", &ticket.tags);
            if let Some(a) = &ticket.assignee {
                println!("  assignee: {a}");
            }
            let body = ticket.body().trim_end();
            if !body.trim().is_empty() {
                println!("\n{body}");
            }
            if !comments.is_empty() {
                println!("\n## Comments");
                for c in &comments {
                    println!("\n— {} ({}):", c.by.as_deref().unwrap_or("?"), c.id);
                    println!("{}", c.body);
                }
            }
            Ok(())
        }
    }
}

/// `comment add` — append a comment to a ticket (one conflict-free file per comment).
pub fn comment_add(repo: &Path, fmt: Format, args: &CommentAddArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let body = body_input(args.body.as_deref(), args.body_file.as_deref())?
        .ok_or_else(|| Error::Invalid("provide --body or --body-file".into()))?;
    let comment = store.add_comment(&args.id, args.as_.clone(), args.reply_to.clone(), &body)?;
    // Best-effort live doorbell for watchers: an event ref in .git, visible
    // cross-worktree without waiting for the comment file to be committed. A
    // no-git repo (the doorbell is auxiliary) just skips it.
    let _ = store.emit_event(
        "comment",
        &args.id,
        comment.by.as_deref(),
        json!({ "comment_id": comment.id, "reply_to": comment.reply_to, "body": comment.body }),
    );
    match fmt {
        Format::Json => {
            let mut v = comment_value(&comment);
            v["schema_version"] = json!(1);
            v["ticket"] = json!(args.id);
            print_json(&v)
        }
        Format::Human => {
            println!("Added comment {} to `{}`", comment.id, args.id);
            Ok(())
        }
    }
}

/// `comment list` — a ticket's comments, from the working tree or a git ref.
pub fn comment_list(repo: &Path, fmt: Format, args: &CommentListArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let comments = match &args.r#ref {
        Some(git_ref) => store.comments_at_ref(&args.id, git_ref)?,
        None => {
            // Working-tree read: surface a typo'd ticket id as not-found.
            store.load(&args.id)?;
            store.comments(&args.id)?
        }
    };
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "ticket": args.id,
            "comments": comments.iter().map(comment_value).collect::<Vec<_>>(),
        })),
        Format::Human => {
            for c in &comments {
                println!("— {} ({}):", c.by.as_deref().unwrap_or("?"), c.id);
                println!("{}\n", c.body);
            }
            Ok(())
        }
    }
}

fn comment_value(c: &Comment) -> Value {
    json!({
        "id": c.id,
        "by": c.by,
        "at": c.at,
        "reply_to": c.reply_to,
        "body": c.body,
    })
}

/// `events` — the cross-branch activity log, filterable and resumable via `--since`.
/// With `--watch`, blocks until at least one matching event appears (exit 7 on
/// timeout) — a wake-on-event the orchestrator loops, advancing `--since`.
pub fn events(repo: &Path, fmt: Format, args: &EventsArgs) -> Result<()> {
    let store = Store::open(repo)?;
    if !args.watch {
        let evs = filter_events(store.events()?, args);
        return print_events(fmt, &evs);
    }
    let start = Instant::now();
    loop {
        let evs = filter_events(store.events()?, args);
        if !evs.is_empty() {
            return print_events(fmt, &evs);
        }
        if let Some(timeout) = args.timeout {
            if start.elapsed().as_secs() >= timeout {
                // Emit an empty payload so stdout always carries JSON, then signal 7.
                print_events(fmt, &[])?;
                return Err(Error::Timeout(format!(
                    "no matching event within {timeout}s"
                )));
            }
        }
        std::thread::sleep(Duration::from_secs(args.interval));
    }
}

fn filter_events(mut evs: Vec<Event>, args: &EventsArgs) -> Vec<Event> {
    if let Some(since) = &args.since {
        evs.retain(|e| &e.id > since);
    }
    if let Some(ticket) = &args.ticket {
        evs.retain(|e| &e.ticket == ticket);
    }
    if let Some(kind) = &args.kind {
        evs.retain(|e| &e.kind == kind);
    }
    evs
}

fn print_events(fmt: Format, evs: &[Event]) -> Result<()> {
    match fmt {
        Format::Json => {
            let events_json = serde_json::to_value(evs)
                .map_err(|e| Error::Internal(format!("serializing events: {e}")))?;
            print_json(&json!({ "schema_version": 1, "events": events_json }))
        }
        Format::Human => {
            for e in evs {
                println!(
                    "{}  {:<8} {}  {}",
                    e.id,
                    e.kind,
                    e.ticket,
                    e.by.as_deref().unwrap_or("")
                );
            }
            Ok(())
        }
    }
}

/// `list` — list tickets, optionally filtered by status.
pub fn list(repo: &Path, fmt: Format, args: &ListArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let status = args
        .status
        .as_deref()
        .map(str::parse::<Status>)
        .transpose()?;
    let priority = args
        .priority
        .as_deref()
        .map(str::parse::<Priority>)
        .transpose()?;
    let (all, warnings) = store.load_all_lenient()?;
    let tickets: Vec<Ticket> = all
        .into_iter()
        .filter(|t| status.map_or(true, |f| t.status == f))
        .filter(|t| priority.map_or(true, |p| t.priority == p))
        .filter(|t| args.scope.as_ref().map_or(true, |s| t.scopes.contains(s)))
        .filter(|t| args.tag.as_ref().map_or(true, |tg| t.tags.contains(tg)))
        .collect();

    match fmt {
        Format::Json => {
            let rows: Vec<Value> = tickets.iter().map(ticket_summary).collect();
            print_json(&json!({
                "schema_version": 1,
                "tickets": rows,
                "warnings": warnings,
            }))
        }
        Format::Human => {
            if tickets.is_empty() {
                println!("(no matching tickets)");
            } else {
                let w = tickets.iter().map(|t| t.id.len()).max().unwrap_or(0);
                for t in &tickets {
                    println!(
                        "{:<3} {:<12} {:<w$}  {}",
                        t.priority.as_str(),
                        t.status.as_str(),
                        t.id,
                        t.title
                    );
                }
                println!("({} ticket(s))", tickets.len());
            }
            for warn in &warnings {
                eprintln!("warning: skipped {warn}");
            }
            Ok(())
        }
    }
}

/// `status` — report ticket status. With `--all-branches`, read each ticket as
/// committed on its `<prefix>*` branch tip; otherwise from the working tree.
pub fn status(repo: &Path, fmt: Format, args: &StatusArgs) -> Result<()> {
    let store = Store::open(repo)?;
    if !args.all_branches {
        let (tickets, warnings) = store.load_all_lenient()?;
        return match fmt {
            Format::Json => {
                let rows: Vec<Value> = tickets
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "status": t.status.as_str(),
                            "assignee": t.assignee,
                            "lease_expires_at": t.lease_expires_at,
                        })
                    })
                    .collect();
                print_json(&json!({
                    "schema_version": 1,
                    "source": "worktree",
                    "tickets": rows,
                    "warnings": warnings,
                }))
            }
            Format::Human => {
                if tickets.is_empty() {
                    println!("(no tickets)");
                }
                for t in &tickets {
                    println!("{:<12} {}", t.status.as_str(), t.id);
                }
                for warn in &warnings {
                    eprintln!("warning: skipped {warn}");
                }
                Ok(())
            }
        };
    }

    let pattern = format!("refs/heads/{}*", args.prefix);
    let branches = git_lines(
        repo,
        &["for-each-ref", "--format=%(refname:short)", &pattern],
    )?;
    let mut rows = Vec::new();
    for branch in &branches {
        let id = branch
            .strip_prefix(&args.prefix)
            .unwrap_or(branch)
            .to_string();
        match store.load_at_ref(&id, branch) {
            Ok(t) => rows.push(json!({
                "branch": branch,
                "id": t.id,
                "status": t.status.as_str(),
                "assignee": t.assignee,
                "lease_expires_at": t.lease_expires_at,
            })),
            // The ticket file may be absent on this branch tip — report, don't abort.
            Err(_) => rows.push(json!({
                "branch": branch,
                "id": id,
                "status": Value::Null,
                "note": "ticket not found on branch tip",
            })),
        }
    }
    match fmt {
        Format::Json => {
            print_json(&json!({ "schema_version": 1, "source": "branches", "tickets": rows }))
        }
        Format::Human => {
            if rows.is_empty() {
                println!("(no {}* branches)", args.prefix);
            }
            for r in &rows {
                println!(
                    "{:<24} {:<12} {}",
                    r["branch"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or("(missing)"),
                    r["id"].as_str().unwrap_or(""),
                );
            }
            Ok(())
        }
    }
}

/// `watch` — block until a ticket reaches `--until` (or `done`), polling the
/// working tree or a git ref. Times out (exit 7) when `--timeout` elapses.
pub fn watch(repo: &Path, fmt: Format, args: &WatchArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let target: Status = args.until.parse()?;
    // Resolve which ref to poll: explicit --ref, else the conventional
    // `<prefix><id>` branch if it exists, else the working tree.
    let resolved_ref: Option<String> = match &args.r#ref {
        Some(r) => Some(r.clone()),
        None => {
            let candidate = format!("{}{}", args.prefix, args.id);
            branch_exists(repo, &candidate).then_some(candidate)
        }
    };
    let start = Instant::now();
    loop {
        let ticket = match &resolved_ref {
            Some(r) => store.load_at_ref(&args.id, r)?,
            None => store.load(&args.id)?,
        };
        // `done` is always terminal, so a ticket that skips past the target still ends the wait.
        if ticket.status == target || ticket.status == Status::Done {
            emit_watch(fmt, args, resolved_ref.as_deref(), ticket.status, true)?;
            return Ok(());
        }
        if let Some(timeout) = args.timeout {
            if start.elapsed().as_secs() >= timeout {
                emit_watch(fmt, args, resolved_ref.as_deref(), ticket.status, false)?;
                return Err(Error::Timeout(format!(
                    "ticket `{}` did not reach `{}` within {timeout}s",
                    args.id, args.until
                )));
            }
        }
        std::thread::sleep(Duration::from_secs(args.interval));
    }
}

/// Emit the watch result (so stdout carries a payload even on the timeout path).
/// `reached` is the only outcome bit: a non-reached emit is always a timeout.
fn emit_watch(
    fmt: Format,
    args: &WatchArgs,
    git_ref: Option<&str>,
    status: Status,
    reached: bool,
) -> Result<()> {
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": args.id,
            "ref": git_ref,
            "status": status.as_str(),
            "reached": reached,
            "timed_out": !reached,
        })),
        Format::Human => {
            let location = git_ref.unwrap_or("(working tree)");
            if reached {
                println!("{} reached `{}` (at {location})", args.id, status.as_str());
            } else {
                println!(
                    "{} timed out at `{}` (at {location})",
                    args.id,
                    status.as_str()
                );
            }
            Ok(())
        }
    }
}

/// Fail with a clean message when `repo` is not inside a git work tree, instead of
/// letting a downstream `git diff`/`git show` dump its multi-line usage and bury the
/// real cause. Mirrors the precondition `claim`'s ref-mutex already relies on.
fn ensure_git_repo(repo: &Path) -> Result<()> {
    let inside = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if inside {
        Ok(())
    } else {
        Err(Error::Invalid(
            "this command requires a git repository (run `git init` and make at least one commit)"
                .to_string(),
        ))
    }
}

/// Whether a local branch exists.
fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run `git <args>` in `repo` and return its non-empty stdout lines.
fn git_lines(repo: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Invalid(format!(
            "git {args:?} failed: {}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// `lint` — schema validation across all tickets. Exits non-zero on findings.
pub fn lint(repo: &Path, fmt: Format) -> Result<()> {
    let store = Store::open(repo)?;
    let mut diagnostics = lint_core::lint(&store)?;
    // One-shot: validate links (dangling deps, cycles) on the parseable subset even
    // when some files fail to parse, so all problem classes surface in one run.
    let (parseable, _) = store.load_all_lenient()?;
    diagnostics.extend(schedule::link_diagnostics(&parseable));
    let problems = diagnostics.len();

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "ok": problems == 0,
            "diagnostics": diagnostics,
        }))?,
        Format::Human => {
            if diagnostics.is_empty() {
                println!("ok: no problems found");
            } else {
                for d in &diagnostics {
                    match &d.id {
                        Some(id) => println!("{} ({id}) [{}]: {}", d.file, d.code, d.message),
                        None => println!("{} [{}]: {}", d.file, d.code, d.message),
                    }
                }
            }
        }
    }

    if problems == 0 {
        Ok(())
    } else if diagnostics.iter().any(|d| d.code == "cycle") {
        // A cycle is exit 5, matching `ready`/`tracks` and the contract.
        Err(Error::Cycle(format!(
            "{problems} problem(s) found, including a dependency cycle"
        )))
    } else {
        Err(Error::Invalid(format!("{problems} problem(s) found")))
    }
}

/// `ready` — the dependency-satisfied, priority-ordered queue.
pub fn ready(repo: &Path, fmt: Format) -> Result<()> {
    let store = Store::open(repo)?;
    let tickets = store.load_all()?;
    let ready = schedule::ready(&tickets)?;
    match fmt {
        Format::Json => {
            let rows: Vec<Value> = ready.iter().map(|t| ticket_summary(t)).collect();
            print_json(&json!({ "schema_version": 1, "ready": rows }))
        }
        Format::Human => {
            if ready.is_empty() {
                println!("(no ready tickets — none are todo/ready with all dependencies done)");
            }
            for t in &ready {
                println!(
                    "{:<3} {:<12} {}  {}",
                    t.priority.as_str(),
                    t.status.as_str(),
                    t.id,
                    t.title
                );
            }
            Ok(())
        }
    }
}

/// `tracks` — conflict-free parallel batches of ready tickets.
pub fn tracks(repo: &Path, fmt: Format) -> Result<()> {
    let store = Store::open(repo)?;
    let tickets = store.load_all()?;
    let batches = schedule::tracks(&tickets)?;
    match fmt {
        Format::Json => {
            let arr: Vec<Value> = batches
                .iter()
                .map(|b| Value::Array(b.iter().map(|t| ticket_summary(t)).collect()))
                .collect();
            print_json(&json!({ "schema_version": 1, "batches": arr }))
        }
        Format::Human => {
            if batches.is_empty() {
                println!("(no ready tickets)");
            }
            for (i, batch) in batches.iter().enumerate() {
                let ids: Vec<&str> = batch.iter().map(|t| t.id.as_str()).collect();
                println!("batch {}: {}", i + 1, ids.join(", "));
            }
            Ok(())
        }
    }
}

/// `next` — scored recommendation(s); `--parallel N` returns N disjoint picks.
pub fn next(repo: &Path, fmt: Format, args: &NextArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let tickets = store.load_all()?;
    let picks = schedule::next(&tickets, args.parallel, args.allow_overlap)?;

    // Atomic dispatch: claim the first pick still free. Trying picks in order makes a
    // lost race (another worker grabbed the top pick) fall through to the next instead
    // of forcing the caller to re-run `next`.
    if args.claim {
        let agent = args
            .agent
            .as_deref()
            .ok_or_else(|| Error::Invalid("next --claim requires --as <worker>".to_string()))?;
        for p in &picks {
            match claim_core::claim(&store, &p.ticket.id, agent, args.ttl, false) {
                Ok(outcome) => {
                    emit_claim_event(&store, &outcome);
                    return print_claim(fmt, &outcome);
                }
                // Raced away or became unclaimable — try the next pick.
                Err(Error::Conflict(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        return match fmt {
            Format::Json => print_json(&json!({ "schema_version": 1, "claimed": Value::Null })),
            Format::Human => {
                println!("(nothing available to claim)");
                Ok(())
            }
        };
    }

    match fmt {
        Format::Json => {
            let rows: Vec<Value> = picks
                .iter()
                .map(|p| {
                    let mut v = ticket_summary(p.ticket);
                    v["score"] = json!(p.score);
                    v["conflicts_with"] = json!(p
                        .conflicts_with
                        .iter()
                        .map(|c| json!({ "ticket": c.ticket, "scopes": c.scopes }))
                        .collect::<Vec<_>>());
                    v
                })
                .collect();
            print_json(&json!({ "schema_version": 1, "picks": rows }))
        }
        Format::Human => {
            if picks.is_empty() {
                println!("(no ready tickets to recommend)");
            }
            for p in &picks {
                println!("{}  (score {})  {}", p.ticket.id, p.score, p.ticket.title);
                for c in &p.conflicts_with {
                    println!("    overlaps `{}` on: {}", c.ticket, c.scopes.join(", "));
                }
            }
            Ok(())
        }
    }
}

/// `migrate` — bring ticket frontmatter up to the current schema (round-trip-safe).
pub fn migrate(repo: &Path, fmt: Format) -> Result<()> {
    let store = Store::open(repo)?;
    let report = migrate_core::migrate(&store)?;
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "migrated": report.migrated,
            "unchanged": report.unchanged,
        })),
        Format::Human => {
            if report.migrated.is_empty() {
                println!("All {} ticket(s) already current", report.unchanged);
            } else {
                println!(
                    "Migrated {} ticket(s): {}",
                    report.migrated.len(),
                    report.migrated.join(", ")
                );
            }
            Ok(())
        }
    }
}

/// `guard` — reconcile a branch's actual diff against its ticket's declared scopes.
pub fn guard(repo: &Path, fmt: Format, args: &GuardArgs) -> Result<()> {
    let store = Store::open(repo)?;
    // A non-git dir would otherwise let the downstream `git diff` dump its full usage.
    ensure_git_repo(repo)?;
    let base = args
        .base
        .clone()
        .unwrap_or_else(|| store.config.default_base.clone());

    let mut warnings: Vec<String> = Vec::new();

    // The [scopes] contract is read from a canonical ref (default: the base), not the
    // possibly stale/empty config on the checked-out feature branch — otherwise a
    // branch that dropped the scope map gets a false all-clear. Fall back to the
    // working-tree config only when the ref carries none (e.g. a fresh, uncommitted init).
    let config_ref = args.config_ref.clone().unwrap_or_else(|| base.clone());
    let config = match store.config_at_ref(&config_ref)? {
        Some(c) => c,
        None => {
            warnings.push(format!(
                "no {CONFIG_FILE} on `{config_ref}`; using the working-tree config"
            ));
            store.config.clone()
        }
    };
    if config.scopes.is_empty() {
        warnings.push(
            "[scopes] is empty — guard cannot map changed files to scopes; \
             configure [scopes] in ticketsplease.toml"
                .to_string(),
        );
    }

    // Collision detection needs siblings' real in-flight status, which in the
    // branch-per-ticket flow lives on each ticket's own branch — overlay the tips.
    let (all, _) = store.load_all_cross_branch(&args.prefix)?;
    let target_id = resolve_ticket(args, &all)?;
    // The target's declared scopes are the agent's current declaration: read the
    // working tree (evaluate() self-skips the target in `all`, so a stale copy
    // there is harmless).
    let target = store.load(&target_id)?;

    let diff = guard::BranchDiff::compute(repo, &base, &args.branch)?;

    let path_mapper = guard::PathGlobMapper::new(&config)?;
    let glob_scopes: BTreeSet<String> = config.scopes.keys().cloned().collect();
    // Config can default the reverse-dep walk off (foundational-crate workspaces);
    // --direct-only forces it off per-invocation.
    let direct_only = args.direct_only || !config.language.reverse_dep_expansion;
    let cargo_mapper = if config.language.backend == Backend::Rust {
        Some(CargoMapper::new(
            repo,
            &config.scope_crates,
            &glob_scopes,
            direct_only,
        ))
    } else {
        None
    };
    // External-scope detection is language-agnostic (it reads manifest diffs) and
    // runs even under --direct-only, since a pin bump is a direct change.
    let external_mapper = if config.external_scopes.is_empty() {
        None
    } else {
        Some(guard::ExternalScopeMapper::new(
            repo,
            &base,
            &args.branch,
            &config.external_scopes,
        )?)
    };
    // direct = what the branch physically touches (path globs + external pins) —
    // authoritative for under-declaration. impact = crate-graph reverse-dep
    // expansion — a non-failing signal feeding collisions/affected only.
    let mut direct: Vec<&dyn guard::AffectedSetMapper> = vec![&path_mapper];
    if let Some(em) = &external_mapper {
        direct.push(em);
    }
    let mut impact: Vec<&dyn guard::AffectedSetMapper> = Vec::new();
    if let Some(cm) = &cargo_mapper {
        impact.push(cm);
    }

    let coverage = guard::coverage_globset(&config, &target)?;
    let mappers = guard::Mappers {
        direct: &direct,
        impact: &impact,
    };
    let report = guard::evaluate(&target, &all, diff, &mappers, &coverage)?;

    // Scope-map gaps: a changed file no [scopes] glob covers is invisible to
    // collision detection, so two tickets can both edit it and collide undetected.
    let covered = guard::config_globset(&config)?;
    let uncovered: Vec<&str> = report
        .changed_files
        .iter()
        .filter(|f| !covered.is_match(f.as_str()))
        .map(String::as_str)
        .collect();
    if !uncovered.is_empty() {
        let sample = uncovered
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "{} changed file(s) covered by no scope (e.g. {sample})",
            uncovered.len()
        ));
    }

    match fmt {
        Format::Json => {
            let mut value = serde_json::to_value(&report)
                .map_err(|e| Error::Internal(format!("serializing guard report: {e}")))?;
            if let Value::Object(ref mut map) = value {
                map.insert("schema_version".to_string(), json!(1));
                map.insert("warnings".to_string(), json!(warnings));
            }
            print_json(&value)?;
        }
        Format::Human => {
            print_guard_human(&report);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
        }
    }

    if report.conflict {
        Err(Error::Conflict(format!(
            "branch `{}` escapes ticket `{}`'s declared scopes or collides with an open ticket",
            report.branch, report.ticket
        )))
    } else {
        Ok(())
    }
}

fn resolve_ticket(args: &GuardArgs, all: &[Ticket]) -> Result<String> {
    if let Some(id) = &args.ticket {
        return Ok(id.clone());
    }
    let mut best: Option<&str> = None;
    for t in all {
        if args.branch.contains(t.id.as_str()) {
            let better = match best {
                Some(b) => t.id.len() > b.len(),
                None => true,
            };
            if better {
                best = Some(t.id.as_str());
            }
        }
    }
    best.map(str::to_string).ok_or_else(|| {
        Error::NotFound(format!(
            "no ticket inferred from branch `{}` (pass --ticket)",
            args.branch
        ))
    })
}

fn print_guard_human(report: &guard::GuardReport) {
    println!(
        "ticket {}  ({}...{})",
        report.ticket, report.base, report.branch
    );
    println!("  changed files:   {}", report.changed_files.len());
    println!(
        "  affected scopes: {}",
        join_or_none(&report.affected_scopes)
    );
    let transitive: Vec<&str> = report
        .affected_causes
        .iter()
        .filter(|(_, c)| **c == guard::ScopeCause::Transitive)
        .map(|(s, _)| s.as_str())
        .collect();
    if !transitive.is_empty() {
        // These are reached only via reverse-deps; an additive change can't break them.
        println!(
            "    (transitive via reverse-deps: {})",
            transitive.join(", ")
        );
    }
    println!(
        "  declared scopes: {}",
        join_or_none(&report.declared_scopes)
    );
    if !report.under_declared.is_empty() {
        println!("  UNDER-DECLARED:  {}", report.under_declared.join(", "));
    }
    for c in &report.collisions {
        println!(
            "  COLLISION ({}) with `{}`: {}",
            c.cause.as_str(),
            c.ticket,
            c.scopes.join(", ")
        );
    }
    println!(
        "  verdict: {}",
        if report.conflict { "CONFLICT" } else { "ok" }
    );
    if report.conflict {
        println!(
            "  note: a declared-area overlap, not a proven merge conflict — declare/narrow scope, \
             coordinate with the listed ticket(s), or build+test the merged result before merging."
        );
    }
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

/// Build the config body for `init`: a Rust-seeded config when a cargo workspace
/// is detected, otherwise the default path-glob template. A cargo-metadata failure
/// falls back to the default rather than failing init.
fn build_config(repo: &Path, tickets_dir: &str) -> String {
    if repo.join("Cargo.toml").exists() {
        if let Ok(members) = workspace_members(repo) {
            if !members.is_empty() {
                return build_rust_config(tickets_dir, &members);
            }
        }
    }
    store::default_config_template(tickets_dir)
}

fn build_rust_config(tickets_dir: &str, members: &[WorkspaceMember]) -> String {
    let mut s = format!(
        "schema_version = 1\ntickets_dir = \"{tickets_dir}\"\ndefault_base = \"main\"\n\n\
         [language]\n# Auto-detected a cargo workspace; the guard expands a changed crate\n\
         # through the cargo reverse-dependency graph.\nbackend = \"rust\"\n\
         # reverse_dep_expansion = false  # default true; off = path/crate-only,\n\
         # handy when a foundational crate makes transitive collisions noisy.\n\n[scopes]\n"
    );
    for m in members {
        let glob = if m.rel_dir.is_empty() {
            "src/**".to_string()
        } else {
            format!("{}/**", m.rel_dir)
        };
        s.push_str(&format!("\"{}\" = [\"{glob}\"]\n", m.name));
    }
    s.push_str("\n[scope_crates]\n");
    for m in members {
        s.push_str(&format!("\"{}\" = \"{}\"\n", m.name, m.name));
    }
    s.push_str(
        "\n# Name a forked/external dependency (pinned via `git = … rev = …`) as a scope.\n\
         # The guard flags a branch that bumps the pin (matched by `repo`) or edits an\n\
         # in-tree fork `paths` glob, against tickets declaring the same scope.\n\
         [external_scopes]\n\
         # \"sqlparser-fork\" = { repo = \"tomsanbear/sqlparser\", paths = [] }\n",
    );
    s
}

/// Resolve a body value from either an inline arg or a file (`-` reads stdin).
/// The CLI `body_op` arg-group guarantees at most one of these is set.
fn body_input(text: Option<&str>, file: Option<&str>) -> Result<Option<String>> {
    if let Some(t) = text {
        Ok(Some(t.to_string()))
    } else if let Some(path) = file {
        Ok(Some(read_text(path)?))
    } else {
        Ok(None)
    }
}

/// Normalize a comma-split arg list: trim each token, drop empties and duplicates.
/// (clap's `value_delimiter` splits on commas but keeps surrounding whitespace.)
fn norm_list(items: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let t = item.trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn read_text(path: &str) -> Result<String> {
    if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(Error::Io)?;
        Ok(s)
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| Error::Invalid(format!("cannot read {path}: {e}")))
    }
}

/// `why` — explain whether two tickets can run in parallel, and if not, why.
pub fn why(repo: &Path, fmt: Format, args: &WhyArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let tickets = store.load_all()?;
    let w = schedule::why(&tickets, &args.a, &args.b)?;
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "a": w.a,
            "b": w.b,
            "conflict": w.conflict,
            "shared_scopes": w.shared_scopes,
            "dependency_ordered": w.dependency_ordered,
        }))?,
        Format::Human => {
            if w.conflict {
                let mut reasons = Vec::new();
                if !w.shared_scopes.is_empty() {
                    reasons.push(format!("shared scope(s): {}", w.shared_scopes.join(", ")));
                }
                if w.dependency_ordered {
                    reasons.push("one depends on the other".to_string());
                }
                println!(
                    "`{}` and `{}` cannot share a batch — {}.",
                    w.a,
                    w.b,
                    reasons.join("; ")
                );
            } else {
                println!(
                    "`{}` and `{}` do not conflict — they can run in parallel.",
                    w.a, w.b
                );
            }
        }
    }
    // Exit 6 on conflict so `why a b && ...` gates without parsing (output already printed).
    if w.conflict {
        Err(Error::Conflict(format!(
            "`{}` and `{}` cannot run in parallel",
            w.a, w.b
        )))
    } else {
        Ok(())
    }
}

/// `claim` — atomically take ownership of a ticket via the git-ref lock + lease.
pub fn claim(repo: &Path, fmt: Format, args: &ClaimArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let outcome = claim_core::claim(&store, &args.id, &args.agent, args.ttl, args.force)?;
    emit_claim_event(&store, &outcome);
    print_claim(fmt, &outcome)
}

/// Emit the `claim` doorbell event, unless this was the holder renewing their own
/// claim — a renewal changes no ownership, so logging it only adds reclaim noise.
fn emit_claim_event(store: &Store, outcome: &claim_core::ClaimOutcome) {
    if outcome.renewed {
        return;
    }
    let _ = store.emit_event(
        "claim",
        &outcome.id,
        Some(&outcome.assignee),
        json!({ "stolen": outcome.stolen, "lease_expires_at": outcome.lease_expires_at }),
    );
}

/// Render a claim outcome (shared by `claim` and `next --claim`).
fn print_claim(fmt: Format, outcome: &claim_core::ClaimOutcome) -> Result<()> {
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": outcome.id,
            "assignee": outcome.assignee,
            "lease_expires_at": outcome.lease_expires_at,
            "stolen": outcome.stolen,
            "renewed": outcome.renewed,
        })),
        Format::Human => {
            let note = if outcome.stolen {
                " (took over a prior claim)"
            } else if outcome.renewed {
                " (renewed)"
            } else {
                ""
            };
            println!("Claimed `{}` for `{}`{note}", outcome.id, outcome.assignee);
            Ok(())
        }
    }
}

/// `release` — drop a claim and return the ticket to the ready pool.
pub fn release(repo: &Path, fmt: Format, args: &ReleaseArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let released = claim_core::release(&store, &args.id, args.agent.as_deref(), args.force)?;
    if released {
        let _ = store.emit_event("release", &args.id, args.agent.as_deref(), Value::Null);
    }
    match fmt {
        Format::Json => {
            print_json(&json!({ "schema_version": 1, "id": args.id, "released": released }))
        }
        Format::Human => {
            if released {
                println!("Released `{}`", args.id);
            } else {
                println!("Ticket `{}` was not claimed (nothing to release)", args.id);
            }
            Ok(())
        }
    }
}

/// `claims` — who holds what: assignee, lease expiry, and live/expired state. With
/// `--all-branches`, also surfaces claims recorded on `<prefix>*` branch tips.
pub fn claims(repo: &Path, fmt: Format, args: &ClaimsArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let (tickets, warnings) = if args.all_branches {
        store.load_all_cross_branch(&args.prefix)?
    } else {
        store.load_all_lenient()?
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let claimed: Vec<&Ticket> = tickets.iter().filter(|t| t.assignee.is_some()).collect();
    match fmt {
        Format::Json => {
            let rows: Vec<Value> = claimed
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "assignee": t.assignee,
                        "lease_expires_at": t.lease_expires_at,
                        "live": t.lease_live(now),
                        "status": t.status.as_str(),
                    })
                })
                .collect();
            print_json(&json!({
                "schema_version": 1,
                "claims": rows,
                "warnings": warnings,
            }))
        }
        Format::Human => {
            if claimed.is_empty() {
                println!("(no active claims)");
            }
            for t in &claimed {
                let state = if t.lease_live(now) { "live" } else { "expired" };
                println!(
                    "{:<16} {:<8} {}",
                    t.assignee.as_deref().unwrap_or("?"),
                    state,
                    t.id
                );
            }
            for w in &warnings {
                eprintln!("warning: skipped {w}");
            }
            Ok(())
        }
    }
}

fn ticket_summary(ticket: &Ticket) -> Value {
    json!({
        "id": ticket.id,
        "title": ticket.title,
        "status": ticket.status.as_str(),
        "priority": ticket.priority.as_str(),
        "scopes": ticket.scopes,
        "paths": ticket.paths,
        "dependencies": ticket.dependencies,
        "tags": ticket.tags,
    })
}

fn ticket_json(ticket: &Ticket) -> Value {
    json!({
        "schema_version": 1,
        "id": ticket.id,
        "title": ticket.title,
        "status": ticket.status.as_str(),
        "priority": ticket.priority.as_str(),
        "dependencies": ticket.dependencies,
        "scopes": ticket.scopes,
        "paths": ticket.paths,
        "tags": ticket.tags,
        "assignee": ticket.assignee,
        "lease_expires_at": ticket.lease_expires_at,
    })
}
