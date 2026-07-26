//! Write-time validation for scopes, links, and planned multi-ticket graphs.
//!
//! This is the authoring gate shared by `create` / `set` / `link` and by
//! [`validate_plan`] for batch mutations. Scheduling still uses
//! [`crate::schedule::Graph::build`]; lint still uses
//! [`crate::schedule::link_diagnostics`]. Those three consumers stay separate.

use std::collections::BTreeMap;

use crate::config::{Config, CONFIG_FILE};
use crate::error::{Error, Result};
use crate::plan::{materialize_board, BoardSnapshot, MutationPlan};
use crate::schedule;
use crate::ticket::Ticket;

/// Scope and link fields to validate for a create/set/link write.
#[derive(Debug, Clone, Copy)]
pub struct WriteFields<'a> {
    /// Exclusive scope claims.
    pub scopes: &'a [String],
    /// Shared/additive scope claims.
    pub shared_scopes: &'a [String],
    /// Related ticket ids.
    pub related: &'a [String],
    /// Dependency ticket ids.
    pub dependencies: &'a [String],
}

/// Which checks [`validate_plan`] runs.
#[derive(Debug, Clone, Copy)]
pub struct ValidationOptions {
    /// Undefined scopes, missing targets, self-edges, closed-without-complete deps.
    pub validate_refs: bool,
    /// Dependency cycle check over the post-image board.
    pub validate_cycles: bool,
}

impl ValidationOptions {
    /// Full write gate: refs + cycles.
    #[must_use]
    pub fn full() -> Self {
        Self {
            validate_refs: true,
            validate_cycles: true,
        }
    }

    /// Forward-ref escape (`--no-validate`): skip refs, still check cycles.
    #[must_use]
    pub fn cycles_only() -> Self {
        Self {
            validate_refs: false,
            validate_cycles: true,
        }
    }
}

/// Validate a new or edited ticket's scopes and links at write time — the same
/// vocabulary `lint` enforces, so a bad batch fails at filing instead of surfacing
/// later at the next gate. `known` maps every id considered to exist (on-disk plus any
/// same-batch peers) to its ticket. Every problem is aggregated into one error. The
/// scope check is a no-op when the repo defines no scopes (not using the system).
pub fn validate_ticket_links(
    config: &Config,
    id: &str,
    fields: &WriteFields<'_>,
    known: &BTreeMap<&str, &Ticket>,
) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let defined = config.defined_scopes();
    if !defined.is_empty() {
        for scope in fields.scopes.iter().chain(fields.shared_scopes) {
            if !defined.contains(scope.as_str()) {
                problems.push(format!(
                    "declares scope `{scope}` not defined in {CONFIG_FILE} \
                     ([scopes], [scope_crates], or [external_scopes])"
                ));
            }
        }
    }
    for r in fields.related {
        if r == id {
            problems.push(format!("related link `{r}` points at itself"));
        } else if !known.contains_key(r.as_str()) {
            problems.push(format!("related link points at missing ticket `{r}`"));
        }
    }
    for d in fields.dependencies {
        if d == id {
            problems.push(format!("dependency `{d}` points at itself"));
        } else if let Some(dep) = known.get(d.as_str()) {
            // A live dependency on a ticket closed without completing is a dead end
            // (mirrors lint's `orphaned-by-closed-dep`).
            if dep.is_terminal() && !dep.completes_dependencies() {
                problems.push(format!(
                    "depends on `{d}` which was closed without completing \
                     (re-point, waive, or drop it)"
                ));
            }
        } else {
            problems.push(format!("depends on missing ticket `{d}`"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        let subject = if id.is_empty() {
            "edit".to_string()
        } else {
            format!("ticket `{id}`")
        };
        Err(Error::Invalid(format!(
            "{subject}: {} (pass --no-validate to skip)",
            problems.join("; ")
        )))
    }
}

/// Validate a mutation plan against the board post-image.
///
/// Builds `known` from [`materialize_board`], runs [`validate_ticket_links`] for each
/// planned create when `validate_refs`, then [`schedule::ensure_acyclic`] when
/// `validate_cycles`.
pub fn validate_plan(
    config: &Config,
    snapshot: &BoardSnapshot,
    plan: &MutationPlan,
    opts: ValidationOptions,
) -> Result<()> {
    let post = materialize_board(snapshot, plan)?;
    let known: BTreeMap<&str, &Ticket> = post.iter().map(|t| (t.id.as_str(), t)).collect();

    if opts.validate_refs {
        // Fail-fast per planned create (same as today's per-ticket validate_write).
        // Each call already aggregates that ticket's problems into one Invalid.
        for planned in &plan.planned_creates {
            let t = &planned.ticket_overlay;
            validate_ticket_links(
                config,
                &planned.final_id,
                &WriteFields {
                    scopes: &t.scopes,
                    shared_scopes: &t.shared_scopes,
                    related: &t.related,
                    dependencies: &t.dependencies,
                },
                &known,
            )?;
        }
    }

    if opts.validate_cycles {
        schedule::ensure_acyclic(&post)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{plan_creates, CreateSpec, TicketRenderer};
    use crate::ticket::Priority;

    fn empty_config() -> Config {
        Config {
            schema_version: 1,
            tickets_dir: "tickets".into(),
            required_version: None,
            default_base: "main".into(),
            language: Default::default(),
            scopes: BTreeMap::new(),
            scope_crates: BTreeMap::new(),
            external_scopes: BTreeMap::new(),
            scope_policy: BTreeMap::new(),
            workflow: Default::default(),
            guard: Default::default(),
            maintenance: Default::default(),
            recipes: BTreeMap::new(),
        }
    }

    fn config_with_scope(name: &str) -> Config {
        let mut c = empty_config();
        c.scopes.insert(name.into(), vec![format!("{name}/**")]);
        c
    }

    fn spec(title: &str, id: Option<&str>, deps: &[&str], scopes: &[&str]) -> CreateSpec {
        CreateSpec {
            id: id.map(str::to_string),
            title: title.into(),
            status: "todo".into(),
            priority: Priority::P2,
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            related: vec![],
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
            shared_scopes: vec![],
            paths: vec![],
            tags: vec![],
            body: String::new(),
            template: None,
        }
    }

    #[test]
    fn validate_ticket_links_aggregates_scope_and_missing_dep() {
        let config = config_with_scope("core");
        let known = BTreeMap::new();
        let scopes = vec!["nope".into()];
        let deps = vec!["ghost".into()];
        let err = validate_ticket_links(
            &config,
            "t",
            &WriteFields {
                scopes: &scopes,
                shared_scopes: &[],
                related: &[],
                dependencies: &deps,
            },
            &known,
        )
        .unwrap_err();
        let msg = err.message();
        assert!(msg.contains("nope"), "{msg}");
        assert!(msg.contains("ghost"), "{msg}");
    }

    #[test]
    fn validate_plan_rejects_cycle_among_planned_creates() {
        let snap = BoardSnapshot::default();
        let specs = vec![
            spec("A", Some("a"), &["b"], &[]),
            spec("B", Some("b"), &["a"], &[]),
        ];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        let err =
            validate_plan(&empty_config(), &snap, &plan, ValidationOptions::full()).unwrap_err();
        assert!(matches!(err, Error::Cycle(_)), "{err}");
    }

    #[test]
    fn validate_plan_cycles_only_still_rejects_cycle() {
        let snap = BoardSnapshot::default();
        let specs = vec![
            spec("A", Some("a"), &["b"], &[]),
            spec("B", Some("b"), &["a"], &[]),
        ];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        let err = validate_plan(
            &empty_config(),
            &snap,
            &plan,
            ValidationOptions::cycles_only(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cycle(_)), "{err}");
    }

    #[test]
    fn validate_plan_accepts_intra_batch_dep_chain() {
        let snap = BoardSnapshot::default();
        let specs = vec![
            spec("Root", Some("root"), &[], &[]),
            spec("Leaf", Some("leaf"), &["root"], &[]),
        ];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        validate_plan(&empty_config(), &snap, &plan, ValidationOptions::full()).unwrap();
    }

    #[test]
    fn validate_plan_rejects_undefined_scope_on_planned_create() {
        let snap = BoardSnapshot::default();
        let specs = vec![spec("One", Some("one"), &[], &["dx"])];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        let err = validate_plan(
            &config_with_scope("core"),
            &snap,
            &plan,
            ValidationOptions::full(),
        )
        .unwrap_err();
        assert!(err.message().contains("dx"), "{}", err.message());
    }

    #[test]
    fn validate_plan_refs_off_skips_missing_dep() {
        let snap = BoardSnapshot::default();
        let specs = vec![spec("X", Some("x"), &["ghost"], &[])];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        // cycles_only: missing dep is not a cycle → ok
        validate_plan(
            &empty_config(),
            &snap,
            &plan,
            ValidationOptions::cycles_only(),
        )
        .unwrap();
    }
}
