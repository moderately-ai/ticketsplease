//! Pure multi-ticket planning: freeze final ids, materialize a post-image board.
//!
//! The planning API is intentionally I/O-free. Callers load a [`BoardSnapshot`] from
//! the store, run [`plan_creates`] / later plan builders, validate the post-image,
//! then hand a [`MutationPlan`] to a journaled `Store::commit` (separate modules).
//!
//! Invariants (initiative `mutation-plan`):
//! - One pure allocation freezes final ids before validate / dry-run / commit.
//! - Edge tokens (`depends_on` / `related`) stay author strings — never rewritten.
//! - No two plan elements share a final create id; disk collisions only if content
//!   is byte-identical ([`CreateOutcome::Unchanged`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::store::{self, CreateOutcome};
use crate::ticket::{Priority, Ticket};

/// On-disk board image used for planning.
///
/// `contents_by_id` holds the full file bytes (or rendered string) so content-identical
/// re-creates can resolve to [`CreateOutcome::Unchanged`] without I/O during plan.
#[derive(Debug, Clone, Default)]
pub struct BoardSnapshot {
    /// Parseable tickets currently on the board (typically from a lenient load).
    pub tickets: Vec<Ticket>,
    /// id → full file contents for Unchanged checks.
    pub contents_by_id: BTreeMap<String, String>,
}

impl BoardSnapshot {
    /// Build a snapshot from tickets alone (empty content map — only free ids).
    #[must_use]
    pub fn from_tickets(tickets: Vec<Ticket>) -> Self {
        Self {
            tickets,
            contents_by_id: BTreeMap::new(),
        }
    }

    /// Build a snapshot with known on-disk contents for Unchanged short-circuit.
    #[must_use]
    pub fn with_contents(tickets: Vec<Ticket>, contents_by_id: BTreeMap<String, String>) -> Self {
        Self {
            tickets,
            contents_by_id,
        }
    }

    /// Every id present on the snapshot board.
    #[must_use]
    pub fn ids(&self) -> BTreeSet<&str> {
        self.tickets.iter().map(|t| t.id.as_str()).collect()
    }
}

/// Pure occupancy tracker: snapshot ids + ids reserved earlier in this plan.
///
/// Mirrors the suffix rules of [`store::create_unique_idempotent`] without touching disk.
#[derive(Debug, Clone)]
pub struct IdAllocator {
    /// id → contents when known (snapshot + plan reservations).
    occupied: BTreeMap<String, Option<String>>,
}

impl IdAllocator {
    /// Seed occupancy from a board snapshot.
    #[must_use]
    pub fn from_snapshot(snapshot: &BoardSnapshot) -> Self {
        let mut occupied = BTreeMap::new();
        for t in &snapshot.tickets {
            let contents = snapshot.contents_by_id.get(&t.id).cloned();
            occupied.insert(t.id.clone(), contents);
        }
        // Contents without a parseable ticket still occupy the id (collision surface).
        for (id, contents) in &snapshot.contents_by_id {
            occupied
                .entry(id.clone())
                .or_insert_with(|| Some(contents.clone()));
        }
        Self { occupied }
    }

    /// Whether `id` is already taken (snapshot or earlier reservation).
    #[must_use]
    pub fn is_occupied(&self, id: &str) -> bool {
        self.occupied.contains_key(id)
    }

    /// Known contents for an occupied id, if recorded.
    #[must_use]
    pub fn contents_of(&self, id: &str) -> Option<&str> {
        self.occupied.get(id).and_then(|c| c.as_deref())
    }

    /// Reserve an explicit id.
    ///
    /// - free → [`CreateOutcome::Created`], reserves with `planned_contents`
    /// - occupied with same contents → [`CreateOutcome::Unchanged`]
    /// - occupied with different / unknown contents → [`Error::Invalid`]
    pub fn reserve_exact(&mut self, id: &str, planned_contents: &str) -> Result<CreateOutcome> {
        store::validate_slug(id)?;
        match self.occupied.get(id) {
            None => {
                self.occupied
                    .insert(id.to_string(), Some(planned_contents.to_string()));
                Ok(CreateOutcome::Created)
            }
            Some(Some(existing)) if existing == planned_contents => Ok(CreateOutcome::Unchanged),
            Some(_) => Err(Error::Invalid(format!(
                "ticket `{id}` already exists with different content"
            ))),
        }
    }

    /// Allocate a unique id from `base` (`base`, `base-2`, …), pure.
    ///
    /// Same algorithm as [`store::create_unique_idempotent`]: try each candidate;
    /// content-identical → Unchanged; different content → next suffix; free → Created.
    pub fn allocate_unique(
        &mut self,
        base: &str,
        mut render: impl FnMut(&str) -> Result<String>,
    ) -> Result<(String, String, CreateOutcome)> {
        // Auto-id bases come from slugify and should already be valid; still gate.
        store::validate_slug(base).map_err(|_| {
            Error::Invalid(format!(
                "invalid auto-id base `{base}` (use lowercase letters, digits, and single hyphens)"
            ))
        })?;
        for n in 1u32.. {
            let id = if n == 1 {
                base.to_string()
            } else {
                format!("{base}-{n}")
            };
            let contents = render(&id)?;
            match self.occupied.get(&id) {
                None => {
                    self.occupied.insert(id.clone(), Some(contents.clone()));
                    return Ok((id, contents, CreateOutcome::Created));
                }
                Some(Some(existing)) if existing == &contents => {
                    return Ok((id, contents, CreateOutcome::Unchanged));
                }
                Some(_) => {
                    // Different content or unknown content → try next suffix.
                }
            }
        }
        unreachable!("u32 id-suffix range is effectively unbounded")
    }
}

/// Author-facing create element after parse (serde-free; CLI maps TicketSpec here).
#[derive(Debug, Clone)]
pub struct CreateSpec {
    /// Explicit id when the author set one; otherwise auto-id from title.
    pub id: Option<String>,
    /// Human title (also the auto-id source).
    pub title: String,
    /// Workflow status name.
    pub status: String,
    /// Priority.
    pub priority: Priority,
    /// Dependency ids (author strings; not rewritten by planning).
    pub depends_on: Vec<String>,
    /// Related ids (author strings).
    pub related: Vec<String>,
    /// Exclusive scopes.
    pub scopes: Vec<String>,
    /// Shared/additive scopes.
    pub shared_scopes: Vec<String>,
    /// Path globs.
    pub paths: Vec<String>,
    /// Tags.
    pub tags: Vec<String>,
    /// Body markdown (raw; templates may still need `{{id}}` substitution at final id).
    pub body: String,
    /// Optional body template name (CLI resolves via `.ticketsplease/templates/`).
    /// Core's [`TicketRenderer`] ignores this and uses `body` as-is.
    pub template: Option<String>,
}

/// One planned create with a frozen final id and fully rendered body.
#[derive(Debug, Clone)]
pub struct PlannedCreate {
    /// Index in the input specs slice.
    pub source_index: usize,
    /// Final ticket id that validate/dry-run/commit will use.
    pub final_id: String,
    /// Full file contents to write (or compare for Unchanged).
    pub contents: String,
    /// Whether this plan element would create or is content-identical Unchanged.
    pub outcome: CreateOutcome,
    /// Typed ticket overlay for materialize / validate (body may be empty in overlay).
    pub ticket_overlay: Ticket,
}

/// One durable working-tree mutation in a plan.
#[derive(Debug, Clone)]
pub enum PendingMutation {
    /// Create a new ticket file (or skip if Unchanged).
    Create {
        /// Final id.
        id: String,
        /// Full file contents.
        contents: String,
        /// Created vs content-identical Unchanged.
        outcome: CreateOutcome,
    },
    /// Overwrite an existing ticket with a full post-image body.
    Upsert {
        /// Ticket id.
        id: String,
        /// Full file contents.
        contents: String,
        /// Destination path when known (optional; commit may derive from id).
        path: Option<PathBuf>,
    },
    /// Delete a ticket file by id.
    DeleteTicket {
        /// Ticket id to remove.
        id: String,
    },
    /// Delete an arbitrary path (e.g. comments dir).
    DeletePath {
        /// Absolute or repo-relative path.
        path: PathBuf,
    },
    /// Rename a directory (e.g. comments).
    RenameDir {
        /// Source path.
        from: PathBuf,
        /// Destination path.
        to: PathBuf,
    },
}

/// Optional plan metadata for CLI reporting (rename pairs, bulk counts, …).
#[derive(Debug, Clone, Default)]
pub struct PlanMeta {
    /// Free-form notes for emitters (not consumed by commit).
    pub notes: Vec<String>,
}

/// Fully planned multi-file mutation ready for validate / dry-run / commit.
#[derive(Debug, Clone, Default)]
pub struct MutationPlan {
    /// Ordered durable ops.
    pub mutations: Vec<PendingMutation>,
    /// Stable create report order: `(final_id, outcome)` per planned create.
    pub create_results: Vec<(String, CreateOutcome)>,
    /// Planned create overlays (same order as `create_results`) for materialize.
    pub planned_creates: Vec<PlannedCreate>,
    /// Emitter metadata.
    pub metadata: PlanMeta,
}

/// Render context for binding final ids into create contents.
///
/// CLI supplies a closure that already resolved templates; core only needs the
/// rendered string. This trait object keeps plan I/O-free while allowing
/// `{{id}}` substitution at the final id.
pub trait CreateRenderer {
    /// Render full ticket file contents for `spec` at `final_id`.
    fn render(&mut self, spec: &CreateSpec, final_id: &str) -> Result<String>;
}

/// A renderer that builds contents via [`Ticket::new`] using `spec.body` as-is
/// (templates already expanded by the caller).
#[derive(Debug, Default)]
pub struct TicketRenderer;

impl CreateRenderer for TicketRenderer {
    fn render(&mut self, spec: &CreateSpec, final_id: &str) -> Result<String> {
        Ticket::new(
            final_id,
            &spec.title,
            &spec.status,
            spec.priority,
            &spec.depends_on,
            &spec.related,
            &spec.scopes,
            &spec.shared_scopes,
            &spec.paths,
            &spec.tags,
            &spec.body,
        )
        .map(|t| t.render())
    }
}

/// Closure-based renderer for tests and CLI.
pub struct FnRenderer<F>(pub F)
where
    F: FnMut(&CreateSpec, &str) -> Result<String>;

impl<F> CreateRenderer for FnRenderer<F>
where
    F: FnMut(&CreateSpec, &str) -> Result<String>,
{
    fn render(&mut self, spec: &CreateSpec, final_id: &str) -> Result<String> {
        (self.0)(spec, final_id)
    }
}

/// Plan a batch of creates: freeze final ids, reserve occupancy, build mutations.
///
/// Edge strings in each spec are **not** rewritten. Callers that need graph-safe
/// auto-id peers must set explicit ids on referenced tickets (initiative policy).
pub fn plan_creates(
    snapshot: &BoardSnapshot,
    specs: &[CreateSpec],
    renderer: &mut dyn CreateRenderer,
) -> Result<MutationPlan> {
    let mut allocator = IdAllocator::from_snapshot(snapshot);
    let mut planned_creates = Vec::with_capacity(specs.len());
    let mut mutations = Vec::with_capacity(specs.len());
    let mut create_results = Vec::with_capacity(specs.len());

    for (source_index, spec) in specs.iter().enumerate() {
        let (final_id, contents, outcome) = if let Some(id) = &spec.id {
            store::validate_slug(id)?;
            let contents = renderer.render(spec, id)?;
            let outcome = allocator.reserve_exact(id, &contents)?;
            (id.clone(), contents, outcome)
        } else {
            let base = store::slugify(&spec.title);
            allocator.allocate_unique(&base, |candidate| renderer.render(spec, candidate))?
        };

        let ticket_overlay = Ticket::new(
            &final_id,
            &spec.title,
            &spec.status,
            spec.priority,
            &spec.depends_on,
            &spec.related,
            &spec.scopes,
            &spec.shared_scopes,
            &spec.paths,
            &spec.tags,
            "", // overlay body unused for link/cycle validation
        )?;

        let planned = PlannedCreate {
            source_index,
            final_id: final_id.clone(),
            contents: contents.clone(),
            outcome,
            ticket_overlay,
        };
        mutations.push(PendingMutation::Create {
            id: final_id.clone(),
            contents,
            outcome,
        });
        create_results.push((final_id, outcome));
        planned_creates.push(planned);
    }

    Ok(MutationPlan {
        mutations,
        create_results,
        planned_creates,
        metadata: PlanMeta::default(),
    })
}

/// Build the post-image board: snapshot tickets with planned creates overlaid.
///
/// - Planned create with a new id is appended.
/// - Planned create that is Unchanged (same id already on board) replaces the
///   snapshot ticket with the plan overlay so validation sees the planned edges.
/// - Snapshot tickets not touched by the plan are kept as-is.
pub fn materialize_board(snapshot: &BoardSnapshot, plan: &MutationPlan) -> Result<Vec<Ticket>> {
    let mut by_id: BTreeMap<String, Ticket> = snapshot
        .tickets
        .iter()
        .map(|t| (t.id.clone(), t.clone()))
        .collect();

    for planned in &plan.planned_creates {
        by_id.insert(planned.final_id.clone(), planned.ticket_overlay.clone());
    }

    // Also apply non-create upserts if present (overlay by re-parsing contents).
    for m in &plan.mutations {
        if let PendingMutation::Upsert { id, contents, .. } = m {
            let t = Ticket::parse(contents)?;
            if t.id != *id {
                return Err(Error::Invalid(format!(
                    "upsert contents id `{}` does not match plan id `{id}`",
                    t.id
                )));
            }
            by_id.insert(id.clone(), t);
        }
        if let PendingMutation::DeleteTicket { id } = m {
            by_id.remove(id);
        }
    }

    let mut out: Vec<Ticket> = by_id.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Build a creates-only plan from already-rendered `(id, contents)` pairs with
/// explicit ids (helper for tests / thin single-create).
pub fn plan_exact_creates(
    snapshot: &BoardSnapshot,
    items: &[(String, String)],
) -> Result<MutationPlan> {
    let mut allocator = IdAllocator::from_snapshot(snapshot);
    let mut plan = MutationPlan::default();
    for (i, (id, contents)) in items.iter().enumerate() {
        store::validate_slug(id)?;
        let outcome = allocator.reserve_exact(id, contents)?;
        let ticket_overlay = match Ticket::parse(contents) {
            Ok(t) => t,
            Err(_) => Ticket::new(
                id,
                id,
                "todo",
                Priority::P2,
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                "",
            )?,
        };
        plan.mutations.push(PendingMutation::Create {
            id: id.clone(),
            contents: contents.clone(),
            outcome,
        });
        plan.create_results.push((id.clone(), outcome));
        plan.planned_creates.push(PlannedCreate {
            source_index: i,
            final_id: id.clone(),
            contents: contents.clone(),
            outcome,
            ticket_overlay,
        });
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CreateOutcome;

    fn empty_snapshot() -> BoardSnapshot {
        BoardSnapshot::default()
    }

    fn ticket_file(id: &str, title: &str, deps: &[&str]) -> String {
        let deps: Vec<String> = deps.iter().map(|s| (*s).to_string()).collect();
        Ticket::new(
            id,
            title,
            "todo",
            Priority::P2,
            &deps,
            &[],
            &[],
            &[],
            &[],
            &[],
            "body\n",
        )
        .unwrap()
        .render()
    }

    fn spec(title: &str, id: Option<&str>, deps: &[&str]) -> CreateSpec {
        CreateSpec {
            id: id.map(str::to_string),
            title: title.into(),
            status: "todo".into(),
            priority: Priority::P2,
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            related: vec![],
            scopes: vec![],
            shared_scopes: vec![],
            paths: vec![],
            tags: vec![],
            body: "body\n".into(),
            template: None,
        }
    }

    #[test]
    fn allocate_unique_free_base_is_created() {
        let mut alloc = IdAllocator::from_snapshot(&empty_snapshot());
        let (id, contents, outcome) = alloc
            .allocate_unique("alpha", |id| Ok(format!("content-{id}")))
            .unwrap();
        assert_eq!(id, "alpha");
        assert_eq!(contents, "content-alpha");
        assert_eq!(outcome, CreateOutcome::Created);
        assert!(alloc.is_occupied("alpha"));
    }

    #[test]
    fn allocate_unique_same_content_is_unchanged() {
        let mut contents = BTreeMap::new();
        contents.insert("alpha".into(), "content-alpha".into());
        let snap = BoardSnapshot::with_contents(vec![], contents);
        let mut alloc = IdAllocator::from_snapshot(&snap);
        let (id, _, outcome) = alloc
            .allocate_unique("alpha", |id| Ok(format!("content-{id}")))
            .unwrap();
        assert_eq!(id, "alpha");
        assert_eq!(outcome, CreateOutcome::Unchanged);
    }

    #[test]
    fn allocate_unique_different_content_suffixes() {
        let mut contents = BTreeMap::new();
        contents.insert("alpha".into(), "other".into());
        let snap = BoardSnapshot::with_contents(vec![], contents);
        let mut alloc = IdAllocator::from_snapshot(&snap);
        let (id, _, outcome) = alloc
            .allocate_unique("alpha", |id| Ok(format!("content-{id}")))
            .unwrap();
        assert_eq!(id, "alpha-2");
        assert_eq!(outcome, CreateOutcome::Created);
    }

    #[test]
    fn allocate_unique_chains_suffixes_inside_plan() {
        let mut alloc = IdAllocator::from_snapshot(&empty_snapshot());
        let (a, _, _) = alloc
            .allocate_unique("same", |id| Ok(format!("one-{id}")))
            .unwrap();
        let (b, _, _) = alloc
            .allocate_unique("same", |id| Ok(format!("two-{id}")))
            .unwrap();
        assert_eq!(a, "same");
        assert_eq!(b, "same-2");
    }

    #[test]
    fn reserve_exact_rejects_different_content() {
        let mut contents = BTreeMap::new();
        contents.insert("x".into(), "old".into());
        let snap = BoardSnapshot::with_contents(vec![], contents);
        let mut alloc = IdAllocator::from_snapshot(&snap);
        let err = alloc.reserve_exact("x", "new").unwrap_err();
        assert!(err.message().contains("different content"));
    }

    #[test]
    fn reserve_exact_duplicate_in_plan_second_fails_if_different() {
        let mut alloc = IdAllocator::from_snapshot(&empty_snapshot());
        assert_eq!(
            alloc.reserve_exact("dup", "body-a").unwrap(),
            CreateOutcome::Created
        );
        assert!(alloc.reserve_exact("dup", "body-b").is_err());
        assert_eq!(
            alloc.reserve_exact("dup", "body-a").unwrap(),
            CreateOutcome::Unchanged
        );
    }

    #[test]
    fn plan_creates_freezes_auto_ids_and_keeps_edge_strings() {
        let specs = vec![
            spec("Root", None, &[]),
            spec("Leaf", None, &["root"]), // author edge stays "root"
        ];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&empty_snapshot(), &specs, &mut renderer).unwrap();
        assert_eq!(plan.create_results.len(), 2);
        assert_eq!(plan.create_results[0].0, "root");
        assert_eq!(plan.create_results[1].0, "leaf");
        assert_eq!(plan.create_results[0].1, CreateOutcome::Created);
        // Edge fidelity: depends_on still "root", not rewritten.
        assert_eq!(
            plan.planned_creates[1].ticket_overlay.dependencies,
            vec!["root".to_string()]
        );
    }

    #[test]
    fn plan_creates_suffixes_when_base_occupied_with_different_content() {
        let existing = Ticket::new(
            "root",
            "Existing",
            "todo",
            Priority::P2,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "old\n",
        )
        .unwrap();
        let mut contents = BTreeMap::new();
        contents.insert("root".into(), existing.render());
        let snap = BoardSnapshot::with_contents(vec![existing], contents);
        let specs = vec![spec("Root", None, &[])];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        assert_eq!(plan.create_results[0].0, "root-2");
        assert_eq!(plan.create_results[0].1, CreateOutcome::Created);
    }

    #[test]
    fn plan_creates_duplicate_explicit_ids_different_bodies_err() {
        let specs = vec![
            spec("A", Some("dup"), &[]),
            CreateSpec {
                body: "other body\n".into(),
                ..spec("B", Some("dup"), &[])
            },
        ];
        let mut renderer = TicketRenderer;
        let err = plan_creates(&empty_snapshot(), &specs, &mut renderer).unwrap_err();
        assert!(err.message().contains("different content"));
    }

    #[test]
    fn materialize_board_overlays_planned_creates() {
        let board = Ticket::new(
            "a",
            "A",
            "todo",
            Priority::P2,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "",
        )
        .unwrap();
        let snap = BoardSnapshot::from_tickets(vec![board]);
        let specs = vec![spec("B", Some("b"), &["a"])];
        let mut renderer = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut renderer).unwrap();
        let post = materialize_board(&snap, &plan).unwrap();
        let ids: Vec<_> = post.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        let b = post.iter().find(|t| t.id == "b").unwrap();
        assert_eq!(b.dependencies, vec!["a".to_string()]);
    }

    #[test]
    fn plan_exact_creates_idempotent_unchanged() {
        let contents = ticket_file("x", "X", &[]);
        let mut map = BTreeMap::new();
        map.insert("x".into(), contents.clone());
        let t = Ticket::parse(&contents).unwrap();
        let snap = BoardSnapshot::with_contents(vec![t], map);
        let plan = plan_exact_creates(&snap, &[("x".into(), contents)]).unwrap();
        assert_eq!(plan.create_results[0].1, CreateOutcome::Unchanged);
    }
}
