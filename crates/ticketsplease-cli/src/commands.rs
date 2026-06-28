//! Command handlers. Each emits human-readable text by default and a stable,
//! versioned JSON payload under `--format json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use ticketsplease_cargo::{workspace_members, CargoMapper, WorkspaceMember};
use ticketsplease_core::claim as claim_core;
use ticketsplease_core::comment::Comment;
use ticketsplease_core::config::{Backend, Config, CONFIG_FILE};
use ticketsplease_core::event::Event;
use ticketsplease_core::guard;
use ticketsplease_core::migrate as migrate_core;
use ticketsplease_core::store::{self, CreateOutcome};
use ticketsplease_core::views::Views;
use ticketsplease_core::{
    lint as lint_core, query, schedule, Error, Priority, Result, Status, Store, Ticket,
};

use crate::cli::{
    ClaimArgs, ClaimsArgs, CommentAddArgs, CommentListArgs, CreateArgs, DeleteArgs, EventsArgs,
    GraphArgs, GuardArgs, InitArgs, LinkArgs, ListArgs, NextArgs, PathArgs, ReconcileArgs,
    ReleaseArgs, RenameArgs, RollupArgs, SelfUpdateArgs, SetArgs, ShowArgs, SkillInstallArgs,
    StatusArgs, TracksArgs, ViewSaveArgs, ViewShowArgs, WatchArgs, WhyArgs,
};
use crate::format::{print_json, Format};
use crate::skill;
use crate::templates;
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
    // Seed the example body templates so `create --template` has something to use and
    // the house convention is discoverable.
    let templates_path = templates::install(repo)?.display().to_string();
    let has_git = git_ref_exists(repo, "HEAD");
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "tickets_dir": dir,
            "wrote_config": outcome.wrote_config,
            "skill_installed": skill_path,
            "templates_installed": templates_path,
            "git": has_git,
        })),
        Format::Human => {
            println!("Initialized ticketsplease (tickets dir: {dir})");
            if !outcome.wrote_config {
                println!("(config already present; left unchanged)");
            }
            if let Some(path) = &skill_path {
                println!("Installed Claude skill to {path}");
            }
            println!("Seeded body templates to {templates_path}");
            println!("\nNext steps:");
            println!("  1. Define your [scopes] in {CONFIG_FILE} (scope name -> path globs).");
            println!("  2. Create a ticket:  tkt create --title \"...\" --scope <scope>");
            println!("  3. See the model:    tkt guide");
            if !has_git {
                println!(
                    "\nwarning: not a git repository — claim/guard/status/events/watch need \
                     `git init` and at least one commit."
                );
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
        return create_batch(&store, fmt, from, args.dry_run);
    }
    let title = args
        .title
        .as_deref()
        .ok_or_else(|| Error::Invalid("provide --title or --from".into()))?;
    let status: Status = args.status.parse()?;
    let priority: Priority = args.priority.parse()?;
    let depends_on = norm_list(&args.depends_on);
    let related = norm_list(&args.related);
    let scopes = norm_list(&args.scopes);
    let paths = norm_list(&args.paths);
    let tags = norm_list(&args.tags);

    let build = |id: &str| -> Result<String> {
        let body = resolve_create_body(repo, &args.body, args.template.as_deref(), id, title)?;
        Ticket::new(
            id,
            title,
            status,
            priority,
            &depends_on,
            &related,
            &scopes,
            &paths,
            &tags,
            &body,
        )
        .map(|t| t.render())
    };

    if args.dry_run {
        let id = args.id.clone().unwrap_or_else(|| store::slugify(title));
        build(&id)?; // surface a build error in the preview too
        let outcome = outcome_for_preview(&store, &id);
        return emit_create_results(fmt, &store, &[(id, outcome)], true);
    }

    let (id, outcome) = if let Some(id) = &args.id {
        let contents = build(id)?;
        (id.clone(), store.create_exact(id, &contents)?)
    } else {
        // Auto-id is content-addressed-idempotent, so re-running the same create is a
        // no-op rather than spawning `<slug>-2` (matches batch create).
        store.create_unique_idempotent(&store::slugify(title), build)?
    };

    // Single and batch create share one result shape: a `results` array of
    // per-ticket {id, created, path}. A consumer reads `.results[]` either way.
    emit_create_results(fmt, &store, &[(id, outcome)], false)
}

/// Resolve a new ticket's body: an explicit `--body` wins; otherwise a `--template`
/// is loaded from `.ticketsplease/templates/` and `{{title}}`/`{{id}}`-substituted;
/// otherwise the body is empty. `{{id}}` resolves to the *final* id (so an auto-id
/// batch element gets the right substitution).
fn resolve_create_body(
    repo: &Path,
    body: &str,
    template: Option<&str>,
    id: &str,
    title: &str,
) -> Result<String> {
    if !body.is_empty() {
        Ok(body.to_string())
    } else if let Some(name) = template {
        templates::load(repo, name, id, title)
    } else {
        Ok(String::new())
    }
}

/// Preview outcome for `--dry-run`: a ticket that already exists would be unchanged,
/// otherwise it would be created.
fn outcome_for_preview(store: &Store, id: &str) -> CreateOutcome {
    if store.path_for(id).exists() {
        CreateOutcome::Unchanged
    } else {
        CreateOutcome::Created
    }
}

/// Emit the shared create result envelope for one or many tickets. With `dry_run`,
/// nothing was written and the verbs are conditional.
fn emit_create_results(
    fmt: Format,
    store: &Store,
    items: &[(String, CreateOutcome)],
    dry_run: bool,
) -> Result<()> {
    match fmt {
        Format::Json => {
            let results: Vec<Value> = items
                .iter()
                .map(|(id, outcome)| {
                    json!({
                        "id": id,
                        "created": *outcome == CreateOutcome::Created,
                        "path": store.path_for(id).display().to_string(),
                    })
                })
                .collect();
            print_json(&json!({ "schema_version": 1, "results": results, "dry_run": dry_run }))
        }
        Format::Human => {
            for (id, outcome) in items {
                match (outcome, dry_run) {
                    (CreateOutcome::Created, false) => println!("Created ticket `{id}`"),
                    (CreateOutcome::Created, true) => println!("Would create ticket `{id}`"),
                    (CreateOutcome::Unchanged, false) => {
                        println!("Ticket `{id}` already exists (unchanged)")
                    }
                    (CreateOutcome::Unchanged, true) => {
                        println!("Ticket `{id}` already exists (no change)")
                    }
                }
            }
            if items.len() > 1 {
                let created = items
                    .iter()
                    .filter(|(_, o)| *o == CreateOutcome::Created)
                    .count();
                let verb = if dry_run { "would create" } else { "created" };
                println!("({created} {verb}, {} unchanged)", items.len() - created);
            }
            Ok(())
        }
    }
}

/// One element of a `create --from` batch. Unknown keys are rejected so a typo like
/// `dependson` fails loudly instead of silently dropping the field.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    related: Vec<String>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    template: Option<String>,
}

/// The TOML manifest shape: `[[ticket]]` array-of-tables (also accepts `[[tickets]]`).
/// JSON `--from` is a bare array, so it does not use this wrapper.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default, alias = "tickets")]
    ticket: Vec<TicketSpec>,
}

/// Parse a `--from` manifest into ticket specs, accepting a JSON array or a TOML
/// `[[ticket]]` document. Format is chosen by file extension (`.toml`/`.json`); for
/// stdin or an extensionless path it sniffs — content starting with `[[` is TOML,
/// otherwise JSON (the established stdin default).
fn parse_manifest(from: &str, raw: &str) -> Result<Vec<TicketSpec>> {
    let is_toml =
        from.ends_with(".toml") || (!from.ends_with(".json") && raw.trim_start().starts_with("[["));
    if is_toml {
        toml::from_str::<Manifest>(raw)
            .map(|m| m.ticket)
            .map_err(|e| {
                Error::Invalid(format!(
                    "invalid --from TOML (expected [[ticket]] tables): {e}"
                ))
            })
    } else {
        serde_json::from_str(raw).map_err(|e| {
            Error::Invalid(format!(
                "invalid --from JSON (expected an array of ticket specs): {e}"
            ))
        })
    }
}

/// A batch spec with its fields parsed once (shared by the validate and write passes).
struct ParsedSpec {
    id: Option<String>,
    title: String,
    status: Status,
    priority: Priority,
    depends_on: Vec<String>,
    related: Vec<String>,
    scopes: Vec<String>,
    paths: Vec<String>,
    tags: Vec<String>,
    body: String,
    template: Option<String>,
}

impl ParsedSpec {
    /// Render this spec's ticket contents for a chosen id. `repo` is needed to resolve
    /// a `template` body (with `{{id}}` bound to the final id).
    fn render(&self, repo: &Path, id: &str) -> Result<String> {
        let body =
            resolve_create_body(repo, &self.body, self.template.as_deref(), id, &self.title)?;
        Ticket::new(
            id,
            &self.title,
            self.status,
            self.priority,
            &self.depends_on,
            &self.related,
            &self.scopes,
            &self.paths,
            &self.tags,
            &body,
        )
        .map(|t| t.render())
    }
}

/// Batch-create from a JSON array of specs (file path, or `-` for stdin). The batch
/// is validated in full before any write (a bad element aborts before partial state),
/// auto-ids are content-addressed-idempotent (re-running is a no-op, not a clone),
/// and the result reports created vs unchanged per element.
fn create_batch(store: &Store, fmt: Format, from: &str, dry_run: bool) -> Result<()> {
    let raw = read_text(from)?;
    let raw_specs = parse_manifest(from, &raw)?;

    // Parse every element up front so a bad status/priority aborts before any write.
    let specs: Vec<ParsedSpec> = raw_specs
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            Ok(ParsedSpec {
                status: parse_field(s.status.as_deref().unwrap_or("todo"), i)?,
                priority: parse_field(s.priority.as_deref().unwrap_or("p2"), i)?,
                id: s.id,
                title: s.title,
                depends_on: norm_list(&s.depends_on),
                related: norm_list(&s.related),
                scopes: norm_list(&s.scopes),
                paths: norm_list(&s.paths),
                tags: norm_list(&s.tags),
                body: s.body,
                template: s.template,
            })
        })
        .collect::<Result<_>>()?;

    // Validate pass (no writes): render each ticket, and reject an explicit id that is
    // invalid or already on disk with different content — so the batch is all-or-nothing
    // for these failure modes rather than applying partially.
    for spec in &specs {
        if let Some(id) = &spec.id {
            store::validate_slug(id)?;
            let contents = spec.render(&store.repo_root, id)?;
            let path = store.path_for(id);
            if path.exists() && std::fs::read_to_string(&path).map_err(Error::Io)? != contents {
                return Err(Error::Invalid(format!(
                    "ticket `{id}` already exists with different content"
                )));
            }
        } else {
            // Render at the base id just to surface any render error before writing.
            spec.render(&store.repo_root, &store::slugify(&spec.title))?;
        }
    }

    // Preview without writing: report the would-be outcome per element.
    if dry_run {
        let results: Vec<(String, CreateOutcome)> = specs
            .iter()
            .map(|spec| {
                let id = spec
                    .id
                    .clone()
                    .unwrap_or_else(|| store::slugify(&spec.title));
                let outcome = outcome_for_preview(store, &id);
                (id, outcome)
            })
            .collect();
        return emit_create_results(fmt, store, &results, true);
    }

    // Write pass.
    let mut results = Vec::with_capacity(specs.len());
    for spec in &specs {
        let item = if let Some(id) = &spec.id {
            (
                id.clone(),
                store.create_exact(id, &spec.render(&store.repo_root, id)?)?,
            )
        } else {
            store.create_unique_idempotent(&store::slugify(&spec.title), |id| {
                spec.render(&store.repo_root, id)
            })?
        };
        results.push(item);
    }

    emit_create_results(fmt, store, &results, false)
}

/// Parse a status/priority field, tagging the error with the batch element index.
fn parse_field<T: std::str::FromStr<Err = Error>>(value: &str, index: usize) -> Result<T> {
    value
        .parse()
        .map_err(|e: Error| Error::Invalid(format!("element {index}: {}", e.message())))
}

/// `set` — surgically update a ticket's fields. With an `id`, edits one ticket;
/// with `--where`/`--view`, edits every matching ticket in one operation.
pub fn set(repo: &Path, fmt: Format, args: &SetArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let bulk = args.where_.is_some() || args.view.is_some();
    match (&args.id, bulk) {
        (Some(_), true) => Err(Error::Invalid(
            "pass either an id or --where/--view, not both".into(),
        )),
        (None, false) => Err(Error::Invalid(
            "provide a ticket id, or --where/--view for a bulk edit".into(),
        )),
        (Some(_), false) => set_single(&store, fmt, args),
        (None, true) => set_bulk(repo, &store, fmt, args),
    }
}

/// Apply the field mutations shared by single and bulk `set`, returning whether any
/// dependency was added (so the caller runs one cycle check). `--title` and body
/// edits are single-target only and are handled by [`set_single`], not here.
fn apply_set_field_mutations(ticket: &mut Ticket, args: &SetArgs) -> Result<bool> {
    if let Some(title) = &args.title {
        ticket.set_title(title)?;
    }
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
    for path in norm_list(&args.add_path) {
        ticket.add_path(&path)?;
    }
    for path in norm_list(&args.remove_path) {
        ticket.remove_path(&path)?;
    }
    let mut deps_added = false;
    for dep in norm_list(&args.add_dependency) {
        if dep == ticket.id {
            return Err(Error::Invalid("a ticket cannot depend on itself".into()));
        }
        deps_added |= ticket.add_dependency(&dep)?;
    }
    for dep in norm_list(&args.remove_dependency) {
        ticket.remove_dependency(&dep)?;
    }
    for rel in norm_list(&args.add_related) {
        if rel == ticket.id {
            return Err(Error::Invalid("a ticket cannot relate to itself".into()));
        }
        ticket.add_related(&rel)?;
    }
    for rel in norm_list(&args.remove_related) {
        ticket.remove_related(&rel)?;
    }
    Ok(deps_added)
}

/// Single-ticket `set` (an explicit id). Body edits and title apply here.
fn set_single(store: &Store, fmt: Format, args: &SetArgs) -> Result<()> {
    let id = args.id.as_deref().expect("single set has an id");
    let mut ticket = store.load(id)?;
    let before = ticket.render();
    let status_before = ticket.status;

    let deps_added = apply_set_field_mutations(&mut ticket, args)?;
    // Reject a dependency edit that would close a cycle, exactly like `link`. Related
    // links carry no ordering, so they need no cycle check.
    if deps_added {
        let mut all = store.load_all()?;
        if let Some(slot) = all.iter_mut().find(|t| t.id == ticket.id) {
            *slot = ticket.clone();
        }
        schedule::ensure_acyclic(&all)?;
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
    // --dry-run computes `changed` but writes nothing and fires no event.
    if changed && !args.dry_run {
        store.save(&ticket)?;
        // A status transition is an activity event watchers care about.
        if ticket.status != status_before {
            let _ = store.emit_event(
                "status",
                &ticket.id,
                None,
                json!({ "status": ticket.status.as_str(), "from": status_before.as_str() }),
            );
        }
    }

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": ticket.id,
            "changed": changed,
            "dry_run": args.dry_run,
        })),
        Format::Human => {
            let verb = match (changed, args.dry_run) {
                (true, false) => "Updated",
                (true, true) => "Would update",
                (false, _) => "No change to",
            };
            println!("{verb} `{}`", ticket.id);
            Ok(())
        }
    }
}

/// Bulk `set` (`--where`/`--view`): apply field mutations to every matching ticket.
/// Field edits only — `--title` and body edits are single-target and rejected here.
/// A single cycle check runs over the whole updated set after all edits.
fn set_bulk(repo: &Path, store: &Store, fmt: Format, args: &SetArgs) -> Result<()> {
    if args.title.is_some() {
        return Err(Error::Invalid(
            "--title is single-ticket only (it would set the same title on every match)".into(),
        ));
    }
    if args.body.is_some()
        || args.body_file.is_some()
        || args.append_body.is_some()
        || args.append_body_file.is_some()
    {
        return Err(Error::Invalid(
            "body edits are single-ticket only; not allowed with --where/--view".into(),
        ));
    }
    let predicate = resolve_filter(repo, args.where_.as_deref(), args.view.as_deref())?
        .ok_or_else(|| Error::Invalid("bulk set requires --where or --view".into()))?;

    let mut all = store.load_all()?;
    let mut any_deps_added = false;
    let mut to_save: Vec<usize> = Vec::new();
    let mut events: Vec<(String, Status, Status)> = Vec::new();
    let mut results: Vec<Value> = Vec::new();
    for (i, ticket) in all.iter_mut().enumerate() {
        if !predicate.matches(ticket) {
            continue;
        }
        let before = ticket.render();
        let status_before = ticket.status;
        any_deps_added |= apply_set_field_mutations(ticket, args)?;
        if ticket.status == Status::Done {
            ticket.clear_lease();
        }
        let changed = ticket.render() != before;
        results.push(json!({ "id": ticket.id, "changed": changed }));
        if changed {
            to_save.push(i);
            if ticket.status != status_before {
                events.push((ticket.id.clone(), status_before, ticket.status));
            }
        }
    }
    // One cycle check over the whole edited set, mirroring single `set`/`link`.
    if any_deps_added {
        schedule::ensure_acyclic(&all)?;
    }
    if !args.dry_run {
        for &i in &to_save {
            store.save(&all[i])?;
        }
        for (id, from, to) in &events {
            let _ = store.emit_event(
                "status",
                id,
                None,
                json!({ "status": to.as_str(), "from": from.as_str() }),
            );
        }
    }

    let matched = results.len();
    let changed_count = to_save.len();
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "matched": matched,
            "results": results,
            "dry_run": args.dry_run,
        })),
        Format::Human => {
            let verb = if args.dry_run {
                "would update"
            } else {
                "updated"
            };
            println!("{matched} matched, {changed_count} {verb}");
            Ok(())
        }
    }
}

/// `link` — add or remove a link between tickets. `--depends-on` is a hard,
/// cycle-checked dependency; `--related` is a soft, non-blocking cross-reference
/// that scheduling ignores (and so is never cycle-checked). The CLI arg-group
/// guarantees exactly one target is set.
pub fn link(repo: &Path, fmt: Format, args: &LinkArgs) -> Result<()> {
    let related = args.related.is_some();
    let target = args
        .depends_on
        .as_deref()
        .or(args.related.as_deref())
        .ok_or_else(|| Error::Invalid("provide --depends-on or --related".into()))?;
    let kind = if related { "related" } else { "dependency" };
    if args.id == target {
        return Err(Error::Invalid(format!(
            "a ticket cannot {kind}-link to itself"
        )));
    }
    let store = Store::open(repo)?;
    let mut ticket = store.load(&args.id)?;

    // Removal never validates the target: a dangling reference (its ticket was
    // deleted) must be cleanable without hand-editing the file.
    let changed = match (related, args.remove) {
        (false, false) => ticket.add_dependency(target)?,
        (false, true) => ticket.remove_dependency(target)?,
        (true, false) => ticket.add_related(target)?,
        (true, true) => ticket.remove_related(target)?,
    };

    // Adding a dependency edge that closes a cycle is rejected here (exit 5) rather
    // than left to corrupt the graph until `ready`/`tracks`/`next` trips over it. A
    // dangling target is permitted, mirroring `create --depends-on` — `lint` reports
    // both dangling deps and cycles on the parseable set. Related links carry no
    // ordering, so they are never cycle-checked.
    if changed && !related && !args.remove {
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
        // Keep the established `depends_on` key for the dependency path; the related
        // path reports a `related` key instead (additive — no key is repurposed).
        Format::Json => {
            let key = if related { "related" } else { "depends_on" };
            print_json(&json!({
                "schema_version": 1,
                "id": ticket.id,
                key: target,
                "removed": args.remove,
                "changed": changed,
            }))
        }
        Format::Human => {
            let verb = match (args.remove, related) {
                (false, false) => "Linked",
                (true, false) => "Unlinked",
                (false, true) => "Related",
                (true, true) => "Unrelated",
            };
            let note = if changed { "" } else { " (no change)" };
            println!("{verb} `{}` -> `{target}`{note}", ticket.id);
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
            line("related: ", &ticket.related);
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
                print_comment_thread(&comments, now_epoch());
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
            print_comment_thread(&comments, now_epoch());
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
    // Events live in `.git` refs; a non-git dir would otherwise return empty success
    // forever, so a tailing consumer never learns git is missing.
    ensure_git_repo(repo)?;
    // Validate filters so a typo (`--type bogs`, a ghost ticket) fails loudly rather
    // than silently masking the whole stream as empty.
    if let Some(kind) = &args.kind {
        const KNOWN: [&str; 4] = ["comment", "status", "claim", "release"];
        if !KNOWN.contains(&kind.as_str()) {
            return Err(Error::Invalid(format!(
                "unknown event type `{kind}` (expected one of: {})",
                KNOWN.join(", ")
            )));
        }
    }
    if let Some(ticket) = &args.ticket {
        store.load(ticket)?; // NotFound (exit 4) for a ghost ticket
    }
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
            let now = now_epoch();
            for e in evs {
                println!(
                    "{:<10} {:<8} {}  {}",
                    humanize_epoch(e.at, now),
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
    // `--where`/`--view` are a full boolean expression; they compose (AND) with the
    // single-axis flags, so existing scripts keep working and add power on top.
    let predicate = resolve_filter(repo, args.where_.as_deref(), args.view.as_deref())?;
    let (all, warnings) = store.load_all_lenient()?;
    let tickets: Vec<Ticket> = all
        .into_iter()
        .filter(|t| status.map_or(true, |f| t.status == f))
        .filter(|t| priority.map_or(true, |p| t.priority == p))
        .filter(|t| args.scope.as_ref().map_or(true, |s| t.scopes.contains(s)))
        .filter(|t| args.tag.as_ref().map_or(true, |tg| t.tags.contains(tg)))
        .filter(|t| !args.hide_done || t.status != Status::Done)
        .filter(|t| predicate.as_ref().map_or(true, |p| p.matches(t)))
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

/// Resolve the optional filter predicate from `--where` and/or `--view`. A `--view`
/// names a saved expression (`tkt view save`); when both are given they are ANDed.
/// Shared by `list`, `set --where`, and `rollup`.
fn resolve_filter(
    repo: &Path,
    where_: Option<&str>,
    view: Option<&str>,
) -> Result<Option<query::Predicate>> {
    let mut preds: Vec<query::Predicate> = Vec::new();
    if let Some(name) = view {
        let views = Views::load(repo)?;
        let v = views
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("view `{name}`")))?;
        preds.push(query::parse(&v.where_expr)?);
    }
    if let Some(expr) = where_ {
        preds.push(query::parse(expr)?);
    }
    Ok(preds
        .into_iter()
        .reduce(|a, b| query::Predicate::And(Box::new(a), Box::new(b))))
}

/// `view save` — store (or overwrite) a named filter expression, validating it first.
pub fn view_save(repo: &Path, fmt: Format, args: &ViewSaveArgs) -> Result<()> {
    let mut views = Views::load(repo)?;
    let replaced = views.insert(&args.name, &args.expr)?;
    views.save(repo)?;
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "name": args.name,
            "where": args.expr,
            "replaced": replaced,
        })),
        Format::Human => {
            let verb = if replaced { "Updated" } else { "Saved" };
            println!("{verb} view `{}`", args.name);
            Ok(())
        }
    }
}

/// `view list` — show all saved views.
pub fn view_list(repo: &Path, fmt: Format) -> Result<()> {
    let views = Views::load(repo)?;
    match fmt {
        Format::Json => {
            let rows: Vec<Value> = views
                .all()
                .iter()
                .map(|(name, v)| json!({ "name": name, "where": v.where_expr }))
                .collect();
            print_json(&json!({ "schema_version": 1, "views": rows }))
        }
        Format::Human => {
            if views.all().is_empty() {
                println!("(no saved views)");
            }
            for (name, v) in views.all() {
                println!("{name}: {}", v.where_expr);
            }
            Ok(())
        }
    }
}

/// `view show` — print a single view's expression (NotFound if absent).
pub fn view_show(repo: &Path, fmt: Format, args: &ViewShowArgs) -> Result<()> {
    let views = Views::load(repo)?;
    let v = views
        .get(&args.name)
        .ok_or_else(|| Error::NotFound(format!("view `{}`", args.name)))?;
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "name": args.name,
            "where": v.where_expr,
        })),
        Format::Human => {
            println!("{}", v.where_expr);
            Ok(())
        }
    }
}

/// `view delete` — remove a saved view (NotFound if absent).
pub fn view_delete(repo: &Path, fmt: Format, args: &ViewShowArgs) -> Result<()> {
    let mut views = Views::load(repo)?;
    if !views.remove(&args.name) {
        return Err(Error::NotFound(format!("view `{}`", args.name)));
    }
    views.save(repo)?;
    match fmt {
        Format::Json => {
            print_json(&json!({ "schema_version": 1, "name": args.name, "deleted": true }))
        }
        Format::Human => {
            println!("Deleted view `{}`", args.name);
            Ok(())
        }
    }
}

/// `rollup` — aggregate an initiative (a tag and/or filter): status & priority
/// counts, percent done, the ready frontier, and the blocked set. Readiness is
/// computed over the **full** board (so a dependency outside the selection is still
/// honoured) and then intersected with the selection. No selector = the whole board.
pub fn rollup(repo: &Path, fmt: Format, args: &RollupArgs) -> Result<()> {
    let store = Store::open(repo)?;
    // Strict load: rollup reports readiness, which needs a valid (acyclic) graph.
    let all = store.load_all()?;
    let predicate = resolve_filter(repo, args.where_.as_deref(), args.view.as_deref())?;
    let selected: Vec<&Ticket> = all
        .iter()
        .filter(|t| args.tag.as_ref().map_or(true, |tag| t.tags.contains(tag)))
        .filter(|t| predicate.as_ref().map_or(true, |p| p.matches(t)))
        .collect();

    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_priority: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &selected {
        *by_status.entry(t.status.as_str()).or_default() += 1;
        *by_priority.entry(t.priority.as_str()).or_default() += 1;
    }
    let total = selected.len();
    let done = selected.iter().filter(|t| t.status == Status::Done).count();
    let percent_done = (done * 100).checked_div(total).unwrap_or(0);

    // Ready frontier: dispatchable over the full board ∩ the selection.
    let ready_ids: BTreeSet<&str> = schedule::ready(&all)?
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    let ready: Vec<&&Ticket> = selected
        .iter()
        .filter(|t| ready_ids.contains(t.id.as_str()))
        .collect();

    // Blocked: in the selection, dispatchable-status but with ≥1 dependency not done.
    let status_by_id: BTreeMap<&str, Status> =
        all.iter().map(|t| (t.id.as_str(), t.status)).collect();
    let blocked: Vec<(&Ticket, Vec<&str>)> = selected
        .iter()
        .filter_map(|t| {
            if !t.status.is_dispatchable() {
                return None;
            }
            let unmet: Vec<&str> = t
                .dependencies
                .iter()
                .filter(|d| status_by_id.get(d.as_str()).copied() != Some(Status::Done))
                .map(String::as_str)
                .collect();
            (!unmet.is_empty()).then_some((*t, unmet))
        })
        .collect();

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "selector": { "tag": args.tag, "where": args.where_, "view": args.view },
            "total": total,
            "done": done,
            "percent_done": percent_done,
            "by_status": by_status,
            "by_priority": by_priority,
            "ready": ready.iter().map(|t| json!({
                "id": t.id, "title": t.title, "priority": t.priority.as_str(),
            })).collect::<Vec<_>>(),
            "blocked": blocked.iter().map(|(t, unmet)| json!({
                "id": t.id, "title": t.title, "unmet": unmet,
            })).collect::<Vec<_>>(),
        })),
        Format::Human => {
            let scope = match (&args.tag, &args.where_, &args.view) {
                (Some(tag), _, _) => format!("tag={tag}"),
                (None, _, Some(view)) => format!("view={view}"),
                (None, Some(_), None) => "filter".to_string(),
                (None, None, None) => "(whole board)".to_string(),
            };
            println!("initiative {scope}: {total} ticket(s), {done} done ({percent_done}%)");
            // Status counts in lifecycle order, skipping absent buckets.
            let order = [
                Status::Todo,
                Status::Ready,
                Status::InProgress,
                Status::Blocked,
                Status::Review,
                Status::Done,
            ];
            let statuses: Vec<String> = order
                .iter()
                .filter_map(|s| {
                    by_status
                        .get(s.as_str())
                        .map(|n| format!("{} {n}", s.as_str()))
                })
                .collect();
            println!("  status:   {}", join_or_none(&statuses));
            let prios: Vec<String> = ["p0", "p1", "p2", "p3"]
                .iter()
                .filter_map(|p| by_priority.get(*p).map(|n| format!("{p} {n}")))
                .collect();
            println!("  priority: {}", join_or_none(&prios));
            let ready_ids: Vec<String> = ready.iter().map(|t| t.id.clone()).collect();
            println!(
                "  ready ({}): {}",
                ready_ids.len(),
                join_or_none(&ready_ids)
            );
            if blocked.is_empty() {
                println!("  blocked (0): (none)");
            } else {
                println!("  blocked ({}):", blocked.len());
                for (t, unmet) in &blocked {
                    println!("    {}  (waiting on: {})", t.id, unmet.join(", "));
                }
            }
            Ok(())
        }
    }
}

/// `graph` — export the dependency DAG. Scoring metrics are board-global; the
/// optional tag/where/view selectors restrict the emitted subgraph (an induced
/// subgraph: an edge is kept only when both endpoints are selected). `--dot` emits
/// Graphviz (dependencies solid, related links dashed) for visualization.
pub fn graph(repo: &Path, fmt: Format, args: &GraphArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let all = store.load_all()?;
    let predicate = resolve_filter(repo, args.where_.as_deref(), args.view.as_deref())?;
    let selected: BTreeSet<&str> = all
        .iter()
        .filter(|t| args.tag.as_ref().map_or(true, |tag| t.tags.contains(tag)))
        .filter(|t| predicate.as_ref().map_or(true, |p| p.matches(t)))
        .map(|t| t.id.as_str())
        .collect();

    let export = schedule::graph_export(&all)?;
    let nodes: Vec<&schedule::GraphNode> = export
        .nodes
        .iter()
        .filter(|n| selected.contains(n.id.as_str()))
        .collect();
    let dep_edges: Vec<&schedule::GraphEdge> = export
        .edges
        .iter()
        .filter(|e| selected.contains(e.from.as_str()) && selected.contains(e.to.as_str()))
        .collect();
    // Related edges are non-blocking, so they live outside the schedule export; induce
    // them on the selection here.
    let related_edges: Vec<(&str, &str)> = all
        .iter()
        .filter(|t| selected.contains(t.id.as_str()))
        .flat_map(|t| {
            t.related
                .iter()
                .filter(|r| selected.contains(r.as_str()))
                .map(move |r| (t.id.as_str(), r.as_str()))
        })
        .collect();

    if args.dot {
        println!("digraph tickets {{");
        println!("  rankdir=LR;");
        for n in &nodes {
            // `{:?}` quotes the id and renders the embedded newline as DOT's `\n`.
            println!(
                "  {:?} [label={:?}];",
                n.id,
                format!("{}\n{}", n.id, n.status)
            );
        }
        for e in &dep_edges {
            println!("  {:?} -> {:?};", e.from, e.to);
        }
        for (from, to) in &related_edges {
            println!("  {from:?} -> {to:?} [style=dashed];");
        }
        println!("}}");
        return Ok(());
    }

    match fmt {
        Format::Json => {
            let nodes_json = serde_json::to_value(&nodes)
                .map_err(|e| Error::Internal(format!("serializing graph nodes: {e}")))?;
            let edges_json = serde_json::to_value(&dep_edges)
                .map_err(|e| Error::Internal(format!("serializing graph edges: {e}")))?;
            print_json(&json!({
                "schema_version": 1,
                "nodes": nodes_json,
                "edges": edges_json,
                "related_edges": related_edges
                    .iter()
                    .map(|(from, to)| json!({ "from": from, "to": to }))
                    .collect::<Vec<_>>(),
            }))
        }
        Format::Human => {
            println!(
                "{} node(s), {} dependency edge(s), {} related edge(s)",
                nodes.len(),
                dep_edges.len(),
                related_edges.len()
            );
            for e in &dep_edges {
                println!("  {} -> {}", e.from, e.to);
            }
            for (from, to) in &related_edges {
                println!("  {from} ~ {to} (related)");
            }
            Ok(())
        }
    }
}

/// `path` — the critical prerequisite path to a ticket: the longest chain of
/// dependencies that must complete before it, root-first with each step's status.
pub fn path(repo: &Path, fmt: Format, args: &PathArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let all = store.load_all()?;
    let chain = schedule::longest_prerequisite_path(&all, &args.id)?;
    let by_id: BTreeMap<&str, &Ticket> = all.iter().map(|t| (t.id.as_str(), t)).collect();
    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "id": args.id,
            "length": chain.len(),
            "path": chain.iter().map(|id| {
                let t = by_id.get(id.as_str());
                json!({
                    "id": id,
                    "status": t.map(|t| t.status.as_str()),
                    "title": t.map(|t| t.title.clone()),
                })
            }).collect::<Vec<_>>(),
        })),
        Format::Human => {
            println!("critical path to `{}` ({} step(s)):", args.id, chain.len());
            for id in &chain {
                let st = by_id.get(id.as_str()).map_or("?", |t| t.status.as_str());
                println!("  {st:<12} {id}");
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
pub fn tracks(repo: &Path, fmt: Format, args: &TracksArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let tickets = store.load_all()?;
    let mut batches = schedule::tracks(&tickets)?;
    // --parallel caps each conflict-free batch to N tickets, splitting larger ones so
    // an orchestrator with N workers gets worker-sized fronts. Tickets within a batch
    // are already disjoint, so any chunking preserves conflict-freedom.
    if let Some(n) = args.parallel.filter(|&n| n > 0) {
        batches = batches
            .into_iter()
            .flat_map(|b| b.chunks(n).map(<[&Ticket]>::to_vec).collect::<Vec<_>>())
            .collect();
    }
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
                map.insert(
                    "transitive_only".to_string(),
                    json!(report.transitive_only()),
                );
            }
            print_json(&value)?;
        }
        Format::Human => {
            print_guard_human(&report);
            if report.transitive_only() && !args.ignore_transitive {
                println!("  (every collision is transitive — `--ignore-transitive` would pass)");
            }
            for w in &warnings {
                eprintln!("warning: {w}");
            }
        }
    }

    // `--ignore-transitive` gates on a real conflict only (a direct overlap or an
    // under-declaration); transitive-only collisions stay in the report but pass.
    let gated = if args.ignore_transitive {
        report.has_direct_conflict()
    } else {
        report.conflict
    };
    if gated {
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
    let now = now_epoch();
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
                let lease = t.lease_expires_at.map_or_else(
                    || "no lease".to_string(),
                    |exp| {
                        let rel = humanize_epoch(exp, now);
                        if t.lease_live(now) {
                            format!("live, expires {rel}")
                        } else {
                            format!("expired {rel}")
                        }
                    },
                );
                println!(
                    "{:<16} {:<28} {}",
                    t.assignee.as_deref().unwrap_or("?"),
                    lease,
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

/// `reconcile` — cross-check the board against git reality. Ticket status lives in
/// markdown with no link to whether the `<prefix><id>` work branch (or its worktree)
/// exists, so the board drifts both ways. This reports the drift so an orchestrator
/// can trust (or repair) the board before dispatching.
pub fn reconcile(repo: &Path, fmt: Format, args: &ReconcileArgs) -> Result<()> {
    let store = Store::open(repo)?;
    ensure_git_repo(repo)?;
    let (tickets, _warnings) = store.load_all_lenient()?;
    let ticket_ids: BTreeSet<&str> = tickets.iter().map(|t| t.id.as_str()).collect();

    let pattern = format!("refs/heads/{}*", args.prefix);
    let branch_ids: BTreeSet<String> = git_lines(
        repo,
        &["for-each-ref", "--format=%(refname:short)", &pattern],
    )?
    .iter()
    .map(|b| b.strip_prefix(&args.prefix).unwrap_or(b).to_string())
    .collect();
    let worktrees = worktree_branches(repo)?;
    let has_worktree = |id: &str| worktrees.contains(&format!("{}{id}", args.prefix));

    let mut findings: Vec<Value> = Vec::new();
    for t in &tickets {
        let has_branch = branch_ids.contains(&t.id);
        match t.status {
            // Marked busy, but nothing is actually running.
            Status::InProgress if !has_branch => findings.push(json!({
                "id": t.id,
                "issue": "in-progress-no-branch",
                "status": t.status.as_str(),
                "branch": false,
                "worktree": false,
                "detail": "in-progress but no work branch exists (abandoned or never-started dispatch)",
            })),
            // Live branch, but the board says the work hasn't started.
            Status::Todo | Status::Ready if has_branch => findings.push(json!({
                "id": t.id,
                "issue": "branch-without-active-ticket",
                "status": t.status.as_str(),
                "branch": true,
                "worktree": has_worktree(&t.id),
                "detail": "a work branch exists but the ticket is not in-progress (untracked in-flight work)",
            })),
            _ => {}
        }
    }
    for id in &branch_ids {
        if !ticket_ids.contains(id.as_str()) {
            findings.push(json!({
                "id": id,
                "issue": "orphan-branch",
                "status": Value::Null,
                "branch": true,
                "worktree": has_worktree(id),
                "detail": "a work branch with no matching ticket",
            }));
        }
    }
    findings.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let ok = findings.is_empty();

    match fmt {
        Format::Json => {
            print_json(&json!({ "schema_version": 1, "ok": ok, "findings": findings }))?;
        }
        Format::Human => {
            if ok {
                println!("ok: board matches git ({}* branches)", args.prefix);
            } else {
                for f in &findings {
                    let wt = if f["worktree"] == json!(true) {
                        " [worktree]"
                    } else {
                        ""
                    };
                    println!(
                        "{} {}{wt}: {}",
                        f["issue"].as_str().unwrap_or(""),
                        f["id"].as_str().unwrap_or(""),
                        f["detail"].as_str().unwrap_or(""),
                    );
                }
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{} reconcile finding(s) — the board does not match git",
            findings.len()
        )))
    }
}

/// `delete` — remove a ticket file (and its comments). git history preserves it.
pub fn delete(repo: &Path, fmt: Format, args: &DeleteArgs) -> Result<()> {
    let store = Store::open(repo)?;
    let path = store.path_for(&args.id);
    if !path.exists() {
        return Err(Error::NotFound(args.id.clone()));
    }
    std::fs::remove_file(&path).map_err(Error::Io)?;
    let comments = store.comments_dir(&args.id);
    if comments.exists() {
        std::fs::remove_dir_all(&comments).map_err(Error::Io)?;
    }
    match fmt {
        Format::Json => print_json(&json!({ "schema_version": 1, "id": args.id, "deleted": true })),
        Format::Human => {
            println!("Deleted `{}`", args.id);
            Ok(())
        }
    }
}

/// `rename` — change a ticket's id: write the new file, repoint every dependent, move
/// the comments, then remove the old file. New file first so an interruption never
/// loses the ticket.
pub fn rename(repo: &Path, fmt: Format, args: &RenameArgs) -> Result<()> {
    let store = Store::open(repo)?;
    store::validate_slug(&args.new)?;
    if args.old == args.new {
        return Err(Error::Invalid("old and new ids are the same".into()));
    }
    let mut ticket = store.load(&args.old)?; // NotFound if missing
    if store.path_for(&args.new).exists() {
        return Err(Error::Conflict(format!(
            "ticket `{}` already exists",
            args.new
        )));
    }
    ticket.set_id(&args.new)?;
    store.create_exact(&args.new, &ticket.render())?;

    // Repoint every ticket that depended on the old id.
    let mut repointed = Vec::new();
    for mut t in store.load_all()? {
        if t.id == args.new {
            continue;
        }
        if t.dependencies.iter().any(|d| d == &args.old) {
            t.remove_dependency(&args.old)?;
            t.add_dependency(&args.new)?;
            store.save(&t)?;
            repointed.push(t.id.clone());
        }
    }

    // Move the comments directory, then drop the old file.
    let old_comments = store.comments_dir(&args.old);
    if old_comments.exists() {
        std::fs::rename(&old_comments, store.comments_dir(&args.new)).map_err(Error::Io)?;
    }
    std::fs::remove_file(store.path_for(&args.old)).map_err(Error::Io)?;

    match fmt {
        Format::Json => print_json(&json!({
            "schema_version": 1,
            "old": args.old,
            "new": args.new,
            "repointed": repointed,
        })),
        Format::Human => {
            println!("Renamed `{}` -> `{}`", args.old, args.new);
            if !repointed.is_empty() {
                println!("  repointed dependents: {}", repointed.join(", "));
            }
            Ok(())
        }
    }
}

/// `doctor` — validate repository setup: config present, git repo with a commit,
/// scope globs compile, and the base ref resolves. Exits non-zero on any failure.
pub fn doctor(repo: &Path, fmt: Format) -> Result<()> {
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        checks.push((name.to_string(), ok, detail));
    };

    // Config + scope globs depend on a loadable config.
    let config = Config::load(repo);
    match &config {
        Ok(cfg) => {
            check("config", true, format!("{CONFIG_FILE} loaded"));
            match guard::PathGlobMapper::new(cfg) {
                Ok(_) => check(
                    "scope_globs",
                    true,
                    format!("{} scope(s) compile", cfg.scopes.len()),
                ),
                Err(e) => check("scope_globs", false, e.message()),
            }
            let base = &cfg.default_base;
            let base_ok = git_ref_exists(repo, base);
            check(
                "base_ref",
                base_ok,
                if base_ok {
                    format!("`{base}` resolves")
                } else {
                    format!("base ref `{base}` does not resolve")
                },
            );
        }
        Err(e) => {
            check("config", false, e.message());
            check("scope_globs", false, "skipped (no config)".to_string());
            check("base_ref", false, "skipped (no config)".to_string());
        }
    }

    let in_git = git_ref_exists(repo, "HEAD");
    check(
        "git_repo",
        in_git,
        if in_git {
            "git repo with at least one commit".to_string()
        } else {
            "not a git repo, or no commit yet (run `git init` + commit)".to_string()
        },
    );

    let ok = checks.iter().all(|(_, c, _)| *c);
    match fmt {
        Format::Json => {
            let rows: Vec<Value> = checks
                .iter()
                .map(|(name, c, detail)| json!({ "check": name, "ok": c, "detail": detail }))
                .collect();
            print_json(&json!({ "schema_version": 1, "ok": ok, "checks": rows }))?;
        }
        Format::Human => {
            for (name, c, detail) in &checks {
                println!("{} {name}: {detail}", if *c { "✓" } else { "✗" });
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid("setup checks failed".into()))
    }
}

/// Render comments as a nested thread: replies indented under their parent. An
/// orphan reply (its parent absent on this ref) renders at the top level.
fn print_comment_thread(comments: &[Comment], now: u64) {
    let ids: BTreeSet<&str> = comments.iter().map(|c| c.id.as_str()).collect();
    let mut children: BTreeMap<&str, Vec<&Comment>> = BTreeMap::new();
    let mut roots: Vec<&Comment> = Vec::new();
    for c in comments {
        match c.reply_to.as_deref() {
            Some(parent) if ids.contains(parent) => {
                children.entry(parent).or_default().push(c);
            }
            _ => roots.push(c),
        }
    }
    for r in &roots {
        print_comment_node(r, 0, &children, now);
    }
}

fn print_comment_node(
    c: &Comment,
    depth: usize,
    children: &BTreeMap<&str, Vec<&Comment>>,
    now: u64,
) {
    let indent = "  ".repeat(depth);
    let when =
        c.at.map(|a| format!(" · {}", humanize_epoch(a, now)))
            .unwrap_or_default();
    println!("{indent}— {}{when}:", c.by.as_deref().unwrap_or("?"));
    for line in c.body.lines() {
        println!("{indent}  {line}");
    }
    if let Some(kids) = children.get(c.id.as_str()) {
        for k in kids {
            print_comment_node(k, depth + 1, children, now);
        }
    }
}

/// Current time in epoch seconds (for relative-time rendering).
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A compact relative time for human output: `just now`, `5m ago`, `in 2h`, `3d ago`.
/// Both arguments are epoch seconds.
fn humanize_epoch(at: u64, now: u64) -> String {
    let future = at > now;
    let delta = if future { at - now } else { now - at };
    let mag = if delta < 60 {
        "<1m".to_string()
    } else if delta < 3600 {
        format!("{}m", delta / 60)
    } else if delta < 86_400 {
        format!("{}h", delta / 3600)
    } else {
        format!("{}d", delta / 86_400)
    };
    if future {
        format!("in {mag}")
    } else if delta < 60 {
        "just now".to_string()
    } else {
        format!("{mag} ago")
    }
}

/// The conceptual model, for humans who haven't read the bundled skill.
const GUIDE: &str = "\
ticketsplease — conceptual guide

Tickets are markdown files with YAML frontmatter under your tickets dir. Each has an
id, status (todo/ready/in-progress/blocked/review/done), priority (p0..p3),
dependencies, and scopes.

Scopes are abstract names you map to path globs in ticketsplease.toml ([scopes]).
A ticket declares the scopes it will touch. Two tickets that share a scope conflict
(can't run in parallel); guard uses scopes to catch a branch leaving its lane.

ready    — tickets whose dependencies are all done (the dispatchable queue).
tracks   — partitions ready tickets into conflict-free parallel batches (no two in a
           batch share a scope). `--parallel N` caps each batch to N.
next     — the highest-impact ready pick(s), scored by
           1000 x priority + 10 x critical-path length + count of tickets it unblocks.
           `--parallel N` returns N scope-disjoint picks; `--claim --as <w>` claims one.
why a b  — explains whether two tickets can run in parallel.

guard <branch> — diffs the branch against a base, maps changed files to scopes, and
           fails (exit 6) if the branch touches scopes its ticket didn't declare
           (under-declaration) or overlaps another open ticket (collision).

claim/release/claims — a git-ref lock + frontmatter lease let many agents claim
           tickets race-free. `claims` shows who holds what.

Exit codes: 0 ok · 3 invalid · 4 not-found · 5 cycle · 6 conflict · 7 timeout.
JSON: every command supports --format json with schema_version: 1.

The bundled Claude skill (installed by `tkt init`) has the full workflow guide.";

/// `guide` — print the conceptual model (scopes, tracks, scoring, guard, claims).
pub fn guide(fmt: Format) -> Result<()> {
    match fmt {
        Format::Json => print_json(&json!({ "schema_version": 1, "guide": GUIDE })),
        Format::Human => {
            println!("{GUIDE}");
            Ok(())
        }
    }
}

/// Branch short-names that currently have a git worktree checked out.
fn worktree_branches(repo: &Path) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for line in git_lines(repo, &["worktree", "list", "--porcelain"])? {
        if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            out.insert(branch.to_string());
        }
    }
    Ok(out)
}

/// Whether a git ref (or `HEAD`) resolves in `repo`.
fn git_ref_exists(repo: &Path, git_ref: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", git_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
        "related": ticket.related,
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
        "related": ticket.related,
        "scopes": ticket.scopes,
        "paths": ticket.paths,
        "tags": ticket.tags,
        "assignee": ticket.assignee,
        "lease_expires_at": ticket.lease_expires_at,
    })
}
