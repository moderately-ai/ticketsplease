//! Command handlers. Each emits human-readable text by default and a stable,
//! versioned JSON payload under `--format json`.

use std::path::Path;

use serde_json::{json, Value};
use ticketsplease_core::store::{self, CreateOutcome};
use ticketsplease_core::{lint as lint_core, Error, Priority, Result, Status, Store, Ticket};

use crate::cli::{CreateArgs, InitArgs, LinkArgs, ListArgs, SetArgs, ShowArgs};
use crate::format::{print_json, Format};

/// `init` — scaffold the tickets directory and config.
pub fn init(repo: &Path, fmt: Format, args: &InitArgs) -> Result<()> {
    let outcome = store::init_repo(repo, &args.dir, args.force)?;
    let dir = outcome.tickets_dir.display().to_string();
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "tickets_dir": dir,
            "wrote_config": outcome.wrote_config,
        })),
        Format::Human => {
            println!("Initialized ticketsplease (tickets dir: {dir})");
            if !outcome.wrote_config {
                println!("(config already present; left unchanged)");
            }
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
    let diagnostics = lint_core::lint(&store)?;
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
