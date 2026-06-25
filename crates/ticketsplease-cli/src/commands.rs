//! Command handlers. Each emits human-readable text by default and a stable,
//! versioned JSON payload under `--format json`.

use std::path::Path;

use serde_json::{json, Value};
use ticketsplease_cargo::{workspace_members, CargoMapper, WorkspaceMember};
use ticketsplease_core::config::Backend;
use ticketsplease_core::guard;
use ticketsplease_core::migrate as migrate_core;
use ticketsplease_core::store::{self, CreateOutcome};
use ticketsplease_core::{
    lint as lint_core, schedule, Error, Priority, Result, Status, Store, Ticket,
};

use crate::cli::{
    CreateArgs, GuardArgs, InitArgs, LinkArgs, ListArgs, NextArgs, SelfUpdateArgs, SetArgs,
    ShowArgs, SkillInstallArgs,
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
    let status: Status = args.status.parse()?;
    let priority: Priority = args.priority.parse()?;

    let build = |id: &str| -> Result<String> {
        Ticket::new(
            id,
            &args.title,
            status,
            priority,
            &args.depends_on,
            &args.scopes,
            &args.paths,
            &args.tags,
            &args.body,
        )
        .map(|t| t.render())
    };

    let (id, outcome) = if let Some(id) = &args.id {
        let contents = build(id)?;
        (id.clone(), store.create_exact(id, &contents)?)
    } else {
        let base = store::slugify(&args.title);
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

/// `set` — surgically update a ticket's fields.
pub fn set(repo: &Path, fmt: Format, args: &SetArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let mut ticket = store.load(&args.id)?;
    let before = ticket.render();

    if let Some(status) = &args.status {
        ticket.set_status(status.parse()?)?;
    }
    if let Some(priority) = &args.priority {
        ticket.set_priority(priority.parse()?)?;
    }
    for scope in &args.add_scope {
        ticket.add_scope(scope)?;
    }
    for scope in &args.remove_scope {
        ticket.remove_scope(scope)?;
    }
    for tag in &args.add_tag {
        ticket.add_tag(tag)?;
    }
    for tag in &args.remove_tag {
        ticket.remove_tag(tag)?;
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

    let changed = ticket.render() != before;
    if changed {
        store.save(&ticket)?;
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
    // The dependency target must exist (otherwise the link is dangling).
    let _target = store.load(&args.depends_on)?;

    let changed = if args.remove {
        ticket.remove_dependency(&args.depends_on)?
    } else {
        ticket.add_dependency(&args.depends_on)?
    };
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

/// `show` — print a single ticket.
pub fn show(repo: &Path, fmt: Format, args: &ShowArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let ticket = store.load(&args.id)?;
    match fmt {
        Format::Json => print_json(&ticket_json(&ticket)),
        Format::Human => {
            print!("{}", ticket.render());
            Ok(())
        }
    }
}

/// `list` — list tickets, optionally filtered by status.
pub fn list(repo: &Path, fmt: Format, args: &ListArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let filter = args
        .status
        .as_deref()
        .map(str::parse::<Status>)
        .transpose()?;
    let tickets: Vec<Ticket> = store
        .load_all()?
        .into_iter()
        .filter(|t| match filter {
            Some(f) => t.status == f,
            None => true,
        })
        .collect();

    match fmt {
        Format::Json => {
            let rows: Vec<Value> = tickets
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "status": t.status.as_str(),
                        "priority": t.priority.as_str(),
                    })
                })
                .collect();
            print_json(&json!({ "schema_version": 1, "tickets": rows }))
        }
        Format::Human => {
            for t in &tickets {
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

/// `lint` — schema validation across all tickets. Exits non-zero on findings.
pub fn lint(repo: &Path, fmt: Format) -> Result<()> {
    let store = Store::open(repo)?;
    let mut diagnostics = lint_core::lint(&store)?;
    // If every file parses, also validate links (dangling deps, cycles).
    if let Ok(tickets) = store.load_all() {
        diagnostics.extend(schedule::link_diagnostics(&tickets));
    }
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
                        Some(id) => println!("{} ({id}): {}", d.file, d.message),
                        None => println!("{}: {}", d.file, d.message),
                    }
                }
            }
        }
    }

    if problems == 0 {
        Ok(())
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
    let picks = schedule::next(&tickets, args.parallel)?;
    match fmt {
        Format::Json => {
            let rows: Vec<Value> = picks
                .iter()
                .map(|p| {
                    let mut v = ticket_summary(p.ticket);
                    v["score"] = json!(p.score);
                    v
                })
                .collect();
            print_json(&json!({ "schema_version": 1, "picks": rows }))
        }
        Format::Human => {
            for p in &picks {
                println!("{}  (score {})  {}", p.ticket.id, p.score, p.ticket.title);
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
    let all = store.load_all()?;
    let base = args
        .base
        .clone()
        .unwrap_or_else(|| store.config.default_base.clone());
    let target_id = resolve_ticket(args, &all)?;
    let target = all
        .iter()
        .find(|t| t.id == target_id)
        .ok_or_else(|| Error::NotFound(target_id.clone()))?;

    let diff = guard::BranchDiff::compute(repo, &base, &args.branch)?;

    let path_mapper = guard::PathGlobMapper::new(&store.config)?;
    let cargo_mapper = if store.config.language.backend == Backend::Rust {
        Some(CargoMapper::new(repo, &store.config.scope_crates))
    } else {
        None
    };
    let mut mappers: Vec<&dyn guard::AffectedSetMapper> = vec![&path_mapper];
    if let Some(cm) = &cargo_mapper {
        mappers.push(cm);
    }

    let report = guard::evaluate(target, &all, diff, &mappers)?;

    match fmt {
        Format::Json => {
            let mut value = serde_json::to_value(&report)
                .map_err(|e| Error::Internal(format!("serializing guard report: {e}")))?;
            if let Value::Object(ref mut map) = value {
                map.insert("schema_version".to_string(), json!(1));
            }
            print_json(&value)?;
        }
        Format::Human => print_guard_human(&report),
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
    println!(
        "  declared scopes: {}",
        join_or_none(&report.declared_scopes)
    );
    if !report.under_declared.is_empty() {
        println!("  UNDER-DECLARED:  {}", report.under_declared.join(", "));
    }
    for c in &report.collisions {
        println!("  COLLISION with `{}`: {}", c.ticket, c.scopes.join(", "));
    }
    println!(
        "  verdict: {}",
        if report.conflict { "CONFLICT" } else { "ok" }
    );
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
         # through the cargo reverse-dependency graph.\nbackend = \"rust\"\n\n[scopes]\n"
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

fn ticket_summary(ticket: &Ticket) -> Value {
    json!({
        "id": ticket.id,
        "title": ticket.title,
        "status": ticket.status.as_str(),
        "priority": ticket.priority.as_str(),
        "scopes": ticket.scopes,
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
    })
}
