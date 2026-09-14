//! Per-ticket watches, receipts, and the needs-review detector.
//!
//! A ticket declares `{ on, of, reason }` watches. tkt matches those against the
//! activity event log and the ticket's spec hash; it never interprets `reason` and
//! never runs an action. The worker resolves with `set` / `close` / `ack`.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use yaml_rust2::YamlLoader;

use crate::error::{Error, Result};
use crate::event::Event;
use crate::frontmatter::{self, Document};
use crate::ids;
use crate::states::StateRegistry;
use crate::ticket::{Ticket, Watch, WatchOf};

/// Reason stored on a spec-hash fire (no matching activity event).
pub const HASH_CHANGED_REASON: &str = "this ticket changed since last ack";

/// How far a receipt covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckScope {
    /// Every currently fired subject, and the spec-hash baseline.
    All,
    /// One subject ticket. Does not move the spec-hash baseline.
    Subject(String),
}

impl AckScope {
    /// Whether this receipt covers `ticket` as a fired subject.
    #[must_use]
    pub fn covers(&self, ticket: &str) -> bool {
        match self {
            AckScope::All => true,
            AckScope::Subject(id) => id == ticket,
        }
    }

    /// The explicit subject, when this receipt is scoped.
    #[must_use]
    pub fn as_ticket(&self) -> Option<&str> {
        match self {
            AckScope::All => None,
            AckScope::Subject(id) => Some(id),
        }
    }
}

/// Construction input for [`Ack::new`]. `id` and `at` are assigned at write time.
pub struct AckSpec {
    /// Who looked.
    pub by: String,
    /// Event cursor consumed by this receipt (`None` if hash-only).
    pub against: Option<String>,
    /// [`spec_hash`] stored on the receipt. Scoped acks copy the last unscoped
    /// baseline rather than advancing it.
    pub hash: String,
    /// How far this receipt covers.
    pub scope: AckScope,
    /// Optional note (markdown body).
    pub note: String,
}

/// Append-only receipt that a worker looked at a ticket's fired watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    /// Sortable unique id; also the filename stem.
    pub id: String,
    /// Who looked.
    pub by: String,
    /// Epoch seconds.
    pub at: u64,
    /// Event cursor consumed by this receipt (`None` if hash-only).
    pub against: Option<String>,
    /// [`spec_hash`] of the ticket at ack time.
    pub hash: String,
    /// How far this receipt covers.
    pub scope: AckScope,
    /// Optional note (markdown body).
    pub note: String,
}

impl Ack {
    /// Build a new receipt.
    #[must_use]
    pub fn new(spec: AckSpec) -> Self {
        Self {
            id: ids::new_id(),
            by: spec.by,
            at: ids::now_secs(),
            against: spec.against,
            hash: spec.hash,
            scope: spec.scope,
            note: spec.note,
        }
    }

    /// Parse a receipt file.
    pub fn parse(raw: &str) -> Result<Self> {
        let doc = Document::parse(raw)?;
        let docs = YamlLoader::load_from_str(doc.fm())
            .map_err(|e| Error::Invalid(format!("invalid ack frontmatter: {e}")))?;
        let y = docs
            .first()
            .ok_or_else(|| Error::Invalid("empty ack frontmatter".into()))?;
        let id = y["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("ack missing `id`".into()))?;
        let by = y["by"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("ack missing `by`".into()))?;
        let at = y["at"]
            .as_i64()
            .map(|n| n as u64)
            .or_else(|| y["at"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0);
        let against = y["against"]
            .as_str()
            .map(str::to_string)
            .or_else(|| y["against"].as_i64().map(|n| n.to_string()))
            .filter(|s| !s.is_empty());
        let hash = y["hash"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("ack missing `hash`".into()))?;
        let scope = y["ticket"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| AckScope::Subject(s.to_string()))
            .unwrap_or(AckScope::All);
        Ok(Self {
            id,
            by,
            at,
            against,
            hash,
            scope,
            note: doc.body().trim_end_matches('\n').to_string(),
        })
    }

    /// Serialize to the on-disk file form.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::from("---\n");
        s.push_str(&format!("id: {}\n", self.id));
        s.push_str(&format!("by: {}\n", frontmatter::render_scalar(&self.by)));
        s.push_str(&format!("at: {}\n", self.at));
        if let Some(against) = &self.against {
            s.push_str(&format!(
                "against: {}\n",
                frontmatter::render_scalar(against)
            ));
        }
        s.push_str(&format!("hash: {}\n", self.hash));
        if let Some(ticket) = self.scope.as_ticket() {
            s.push_str(&format!("ticket: {}\n", frontmatter::render_scalar(ticket)));
        }
        s.push_str("---\n");
        if !self.note.is_empty() {
            s.push_str(self.note.trim_end_matches('\n'));
            s.push('\n');
        }
        s
    }
}

/// What `ack` will write (or skip) given current fires and receipts.
pub struct AckPlan {
    /// Event cursor to store (and to report in JSON).
    pub against: Option<String>,
    /// Subject ids this receipt covers.
    pub tickets: Vec<String>,
    /// Nothing new to acknowledge.
    pub caught_up: bool,
    /// Hash written onto the receipt. Unscoped acks store the current spec hash;
    /// scoped acks copy the last unscoped baseline so they cannot clear a hash fire.
    pub stored_hash: String,
    /// The watcher's current spec hash (JSON `hash`; not necessarily `stored_hash`).
    pub current_hash: String,
}

impl AckPlan {
    /// Compute the receipt a subsequent write would store.
    #[must_use]
    pub fn new(
        fired: &[FiredWatch],
        receipts: &[Ack],
        scope: &AckScope,
        current_hash: &str,
    ) -> Self {
        let pending: Vec<&FiredWatch> = fired.iter().filter(|f| scope.covers(f.ticket())).collect();
        let against = pending
            .iter()
            .filter_map(|f| f.event().map(|e| e.id.as_str()))
            .max()
            .map(str::to_string)
            .or_else(|| prior_against(receipts, scope).map(str::to_string));
        let tickets = match scope {
            AckScope::Subject(id) => vec![id.clone()],
            AckScope::All => {
                let mut ids: Vec<String> = pending.iter().map(|f| f.ticket().to_string()).collect();
                ids.sort();
                ids.dedup();
                ids
            }
        };
        let stored_hash = match scope {
            AckScope::All => current_hash.to_string(),
            AckScope::Subject(_) => last_unscoped(receipts)
                .map(|a| a.hash.clone())
                .unwrap_or_else(|| current_hash.to_string()),
        };
        let caught_up = pending.is_empty()
            && match scope {
                AckScope::Subject(_) => true,
                AckScope::All => last_unscoped(receipts).is_some_and(|a| a.hash == current_hash),
            };
        Self {
            against,
            tickets,
            caught_up,
            stored_hash,
            current_hash: current_hash.to_string(),
        }
    }
}

fn prior_against<'a>(receipts: &'a [Ack], scope: &AckScope) -> Option<&'a str> {
    match scope {
        AckScope::All => last_unscoped(receipts).and_then(|a| a.against.as_deref()),
        AckScope::Subject(id) => receipts
            .iter()
            .filter(|a| a.scope.as_ticket() == Some(id.as_str()))
            .max_by_key(|a| a.id.as_str())
            .and_then(|a| a.against.as_deref()),
    }
}

/// A watch that has fired and has not been acked since.
#[derive(Debug, Clone, PartialEq)]
pub enum FiredWatch {
    /// An activity event matched a watch entry.
    Event {
        /// Event kind / status alias.
        on: String,
        /// The watch's `of` binding.
        of: WatchOf,
        /// The subject ticket that produced the event.
        ticket: String,
        /// Why the author is watching.
        reason: String,
        /// The matching activity event. Boxed so `SpecChanged` does not pad out to this size.
        event: Box<Event>,
    },
    /// The watcher's spec hash moved since the last unscoped ack.
    SpecChanged {
        /// The watching ticket.
        ticket: String,
    },
}

impl FiredWatch {
    /// Subject ticket (the event's ticket, or the watcher for a hash-only fire).
    #[must_use]
    pub fn ticket(&self) -> &str {
        match self {
            FiredWatch::Event { ticket, .. } | FiredWatch::SpecChanged { ticket } => ticket,
        }
    }

    /// Author reason, or [`HASH_CHANGED_REASON`].
    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            FiredWatch::Event { reason, .. } => reason,
            FiredWatch::SpecChanged { .. } => HASH_CHANGED_REASON,
        }
    }

    /// Matching activity event; `None` for a spec-hash change.
    #[must_use]
    pub fn event(&self) -> Option<&Event> {
        match self {
            FiredWatch::Event { event, .. } => Some(event.as_ref()),
            FiredWatch::SpecChanged { .. } => None,
        }
    }
}

/// SHA-256 of the ticket's spec — body, title, links, scopes, tags, watch, priority.
/// Claim/status metadata is excluded so `claim` itself does not retrigger review.
#[must_use]
pub fn spec_hash(ticket: &Ticket) -> String {
    let mut h = Sha256::new();
    feed(&mut h, b"", &ticket.id);
    feed(&mut h, b"", &ticket.title);
    feed(&mut h, b"", ticket.priority.as_str());
    for d in &ticket.dependencies {
        feed(&mut h, b"d:", d);
    }
    for r in &ticket.related {
        feed(&mut h, b"r:", r);
    }
    for s in &ticket.scopes {
        feed(&mut h, b"s:", s);
    }
    for s in &ticket.shared_scopes {
        feed(&mut h, b"h:", s);
    }
    for p in &ticket.paths {
        feed(&mut h, b"p:", p);
    }
    for t in &ticket.tags {
        feed(&mut h, b"t:", t);
    }
    for w in &ticket.watch {
        h.update(b"w:");
        h.update(w.on.as_bytes());
        h.update(b":");
        h.update(w.of.as_str().as_bytes());
        h.update(b":");
        h.update(w.reason.as_bytes());
        h.update(b"\0");
    }
    h.update(b"b:");
    h.update(ticket.body().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn feed(h: &mut Sha256, prefix: &[u8], value: &str) {
    h.update(prefix);
    h.update(value.as_bytes());
    h.update(b"\0");
}

/// Whether `on` is a known watch trigger for this registry.
#[must_use]
pub fn is_known_on(on: &str, registry: &StateRegistry) -> bool {
    matches!(on, "claim" | "release" | "reopen" | "done" | "closed") || registry.contains(on)
}

/// Whether `on` is a terminal status alias (cannot usefully watch `self` for it).
#[must_use]
pub fn is_terminal_on(on: &str, registry: &StateRegistry) -> bool {
    matches!(on, "done" | "closed") || registry.class(on).is_terminal()
}

/// Parse `--add-watch on:of` (first colon). Slugs cannot contain `:`.
pub fn parse_on_of(spec: &str) -> Result<(String, WatchOf)> {
    let (on, of) = spec.split_once(':').ok_or_else(|| {
        Error::Invalid(format!(
            "watch spec `{spec}` must be `on:of` (e.g. done:dependencies)"
        ))
    })?;
    let on = on.trim().to_ascii_lowercase();
    let of = of.trim();
    if on.is_empty() || of.is_empty() {
        return Err(Error::Invalid(format!(
            "watch spec `{spec}` must be `on:of` (e.g. done:dependencies)"
        )));
    }
    Ok((on, WatchOf::parse(of)))
}

/// Build a watch from CLI flags, rejecting an empty reason.
pub fn watch_from_spec(spec: &str, reason: &str) -> Result<Watch> {
    let (on, of) = parse_on_of(spec)?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(Error::Invalid(
            "watch reason must be a non-empty sentence".into(),
        ));
    }
    Ok(Watch::new(on, of, reason))
}

/// Whether explicit `of` ids must exist on the board.
#[derive(Debug, Clone, Copy)]
pub enum RefCheck<'a> {
    /// Skip dangling-id checks (`--no-validate`). Unknown `on` still fails.
    Skip,
    /// Explicit `of` must be in this set of ticket ids.
    Known(&'a BTreeSet<&'a str>),
}

/// Shared checks for write-time validation and lint.
pub struct WatchCheck<'a> {
    /// Workflow registry (built-in `on` aliases plus configured states).
    pub registry: &'a StateRegistry,
    /// Whether explicit `of` ids must exist.
    pub refs: RefCheck<'a>,
}

/// One problem with a watch entry. Lint maps [`WatchProblem::lint_code`]; write
/// validation joins [`WatchProblem::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchProblem {
    /// `reason` is empty or whitespace.
    EmptyReason { on: String, of: WatchOf },
    /// `on` is not a built-in alias or a workflow state.
    UnknownOn { on: String },
    /// `of: self` for a terminal status can never fire while the ticket is open.
    SelfTerminal { on: String },
    /// Explicit `of` is not a valid ticket slug.
    BadOf { of: WatchOf },
    /// Explicit `of` names a ticket that is not on the board.
    MissingOf { of: WatchOf },
}

impl WatchProblem {
    /// Stable lint diagnostic code.
    #[must_use]
    pub fn lint_code(&self) -> &'static str {
        match self {
            WatchProblem::EmptyReason { .. } => "empty-watch-reason",
            WatchProblem::UnknownOn { .. } => "unknown-watch-on",
            WatchProblem::SelfTerminal { .. } => "watch-self-terminal",
            WatchProblem::BadOf { .. } => "invalid-watch-of",
            WatchProblem::MissingOf { .. } => "missing-watch-target",
        }
    }

    /// Human-readable problem, without a ticket/config prefix.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            WatchProblem::EmptyReason { on, of } => {
                format!("watch `on: {on}` `of: {of}` has an empty reason")
            }
            WatchProblem::UnknownOn { on } => format!(
                "unknown watch `on` `{on}` (expected claim|release|reopen|done|closed or a workflow state)"
            ),
            WatchProblem::SelfTerminal { on } => {
                format!("watching `self` for `{on}` can never fire while the ticket is open")
            }
            WatchProblem::BadOf { of } => {
                format!("watch `of` `{of}` is not self|dependencies|related or a valid ticket slug")
            }
            WatchProblem::MissingOf { of } => {
                format!("watch `of` points at missing ticket `{of}`")
            }
        }
    }
}

/// Write-time checks for a single watch entry. [`RefCheck::Skip`] omits dangling
/// explicit `of` ids; unknown `on` and empty reason always fail.
pub fn watch_problems(watch: &Watch, check: &WatchCheck<'_>) -> Vec<WatchProblem> {
    let mut problems = Vec::new();
    if watch.reason.trim().is_empty() {
        problems.push(WatchProblem::EmptyReason {
            on: watch.on.clone(),
            of: watch.of.clone(),
        });
    }
    if !is_known_on(&watch.on, check.registry) {
        problems.push(WatchProblem::UnknownOn {
            on: watch.on.clone(),
        });
    }
    if matches!(watch.of, WatchOf::Watcher) && is_terminal_on(&watch.on, check.registry) {
        problems.push(WatchProblem::SelfTerminal {
            on: watch.on.clone(),
        });
    }
    if let WatchOf::Ticket(id) = &watch.of {
        if crate::store::validate_slug(id).is_err() {
            problems.push(WatchProblem::BadOf {
                of: watch.of.clone(),
            });
        } else if let RefCheck::Known(known) = check.refs {
            if !known.contains(id.as_str()) {
                problems.push(WatchProblem::MissingOf {
                    of: watch.of.clone(),
                });
            }
        }
    }
    problems
}

/// [`watch_problems`] as a single error. `subject` is the prefix (`ticket \`foo\``
/// or `[defaults].watch`).
pub fn validate_watch(subject: &str, watch: &Watch, check: &WatchCheck<'_>) -> Result<()> {
    let problems = watch_problems(watch, check);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{subject}: {}",
            problems
                .iter()
                .map(WatchProblem::message)
                .collect::<Vec<_>>()
                .join("; ")
        )))
    }
}

/// Duplicate `(on, of)` on one ticket.
pub fn duplicate_watch(watches: &[Watch]) -> Option<&Watch> {
    let mut seen = BTreeSet::new();
    watches
        .iter()
        .find(|w| !seen.insert((w.on.as_str(), w.of.as_str())))
}

/// Inputs to [`needs_review`].
pub struct ReviewInput<'a> {
    /// The watching ticket.
    pub ticket: &'a Ticket,
    /// The board (inbound `related` needs every ticket).
    pub board: &'a [Ticket],
    /// Activity log, oldest to newest.
    pub events: &'a [Event],
    /// Receipts on `ticket`.
    pub acks: &'a [Ack],
    /// Workflow registry.
    pub registry: &'a StateRegistry,
}

/// Fired watches (and a hash-only item) that have not been acked since.
///
/// Deterministic: event fires sorted by `(ticket, event.id)`, then a hash-only
/// item if the spec hash moved.
#[must_use]
pub fn needs_review(input: ReviewInput<'_>) -> Vec<FiredWatch> {
    let hash = spec_hash(input.ticket);
    let hash_changed = last_unscoped(input.acks).is_some_and(|a| a.hash != hash);

    let mut fired = Vec::new();
    for watch in &input.ticket.watch {
        if matches!(watch.of, WatchOf::Watcher) && is_terminal_on(&watch.on, input.registry) {
            continue;
        }
        for subject in subjects(watch, input.ticket, input.board) {
            let against = cursor_for(input.acks, &subject);
            for event in input.events {
                if event.ticket != subject {
                    continue;
                }
                if event.id.as_str() <= against {
                    continue;
                }
                if !event_matches(&watch.on, event, input.registry) {
                    continue;
                }
                fired.push(FiredWatch::Event {
                    on: watch.on.clone(),
                    of: watch.of.clone(),
                    ticket: subject.clone(),
                    reason: watch.reason.clone(),
                    event: Box::new(event.clone()),
                });
            }
        }
    }

    fired.sort_by(|a, b| {
        a.ticket().cmp(b.ticket()).then_with(|| {
            a.event()
                .map(|e| e.id.as_str())
                .unwrap_or("")
                .cmp(b.event().map(|e| e.id.as_str()).unwrap_or(""))
        })
    });

    if hash_changed {
        fired.push(FiredWatch::SpecChanged {
            ticket: input.ticket.id.clone(),
        });
    }
    fired
}

fn last_unscoped(acks: &[Ack]) -> Option<&Ack> {
    acks.iter()
        .filter(|a| a.scope == AckScope::All)
        .max_by_key(|a| a.id.as_str())
}

/// Latest receipt covering `subject`: an unscoped ack or one named for that ticket.
fn cursor_for<'a>(acks: &'a [Ack], subject: &str) -> &'a str {
    acks.iter()
        .filter(|a| a.scope.covers(subject))
        .max_by_key(|a| a.id.as_str())
        .and_then(|a| a.against.as_deref())
        .unwrap_or("")
}

fn subjects(watch: &Watch, ticket: &Ticket, board: &[Ticket]) -> BTreeSet<String> {
    match &watch.of {
        WatchOf::Watcher => BTreeSet::from([ticket.id.clone()]),
        WatchOf::Dependencies => ticket.dependencies.iter().cloned().collect(),
        WatchOf::Related => {
            let mut s: BTreeSet<String> = ticket.related.iter().cloned().collect();
            for t in board {
                if t.id != ticket.id && t.related.iter().any(|r| r == &ticket.id) {
                    s.insert(t.id.clone());
                }
            }
            s
        }
        WatchOf::Ticket(id) => BTreeSet::from([id.clone()]),
    }
}

fn event_matches(on: &str, event: &Event, registry: &StateRegistry) -> bool {
    match on {
        "claim" => event.kind == "claim",
        "release" => event.kind == "release",
        "reopen" => event.kind == "status" && is_reopen_event(event, registry),
        "done" => {
            event.kind == "status"
                && status_of(event).is_some_and(|s| registry.class(s).completes_dependencies())
        }
        "closed" => {
            event.kind == "status"
                && status_of(event).is_some_and(|s| registry.class(s).is_dropped())
        }
        other => event.kind == "status" && status_of(event) == Some(other),
    }
}

fn status_of(event: &Event) -> Option<&str> {
    event.data.get("status").and_then(|v| v.as_str())
}

fn is_reopen_event(event: &Event, registry: &StateRegistry) -> bool {
    let from = event
        .data
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let to = status_of(event).unwrap_or("");
    registry.class(from).is_terminal() && !registry.class(to).is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticket::Priority;

    fn tkt(id: &str, deps: &[&str], watch: Watch) -> Ticket {
        let deps: Vec<String> = deps.iter().map(|s| (*s).to_string()).collect();
        let mut t = Ticket::new(
            id,
            id,
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
        .unwrap();
        t.set_watch(&[watch]).unwrap();
        t
    }

    fn watch_done_deps() -> Watch {
        Watch::new(
            "done",
            WatchOf::Dependencies,
            "Assumes SHA identity. If that changes, rewrite before implementing.",
        )
    }

    fn review<'a>(
        ticket: &'a Ticket,
        board: &'a [Ticket],
        events: &'a [Event],
        acks: &'a [Ack],
        registry: &'a StateRegistry,
    ) -> Vec<FiredWatch> {
        needs_review(ReviewInput {
            ticket,
            board,
            events,
            acks,
            registry,
        })
    }

    fn status_event(id: &str, ticket: &str, status: &str) -> Event {
        Event {
            id: id.into(),
            ticket: ticket.into(),
            kind: "status".into(),
            by: Some("jules".into()),
            at: 1,
            data: serde_json::json!({ "status": status, "from": "todo" }),
        }
    }

    #[test]
    fn parse_on_of_splits_on_first_colon() {
        assert_eq!(
            parse_on_of("done:github-adapter").unwrap(),
            ("done".into(), WatchOf::Ticket("github-adapter".into()))
        );
        assert_eq!(
            parse_on_of("done:dependencies").unwrap(),
            ("done".into(), WatchOf::Dependencies)
        );
        assert!(parse_on_of("done").is_err());
    }

    #[test]
    fn a_done_fires_c_then_ack_clears_until_b_done() {
        let registry = StateRegistry::builtin();
        let a = Ticket::new(
            "deployment-record",
            "A",
            "done",
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
        let b = Ticket::new(
            "github-adapter",
            "B",
            "todo",
            Priority::P2,
            &["deployment-record".into()],
            &[],
            &[],
            &[],
            &[],
            &[],
            "",
        )
        .unwrap();
        let c = tkt(
            "slack-notify",
            &["deployment-record", "github-adapter"],
            watch_done_deps(),
        );
        let board = [a, b, c.clone()];
        let ev_a = status_event("1", "deployment-record", "done");
        let fired = review(&c, &board, std::slice::from_ref(&ev_a), &[], &registry);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].ticket(), "deployment-record");
        assert_eq!(
            fired[0].reason(),
            "Assumes SHA identity. If that changes, rewrite before implementing."
        );

        let ack = Ack {
            id: "0".into(),
            by: "ren".into(),
            at: 1,
            against: Some("1".into()),
            hash: spec_hash(&c),
            scope: AckScope::All,
            note: String::new(),
        };
        let fired = review(
            &c,
            &board,
            std::slice::from_ref(&ev_a),
            std::slice::from_ref(&ack),
            &registry,
        );
        assert!(fired.is_empty(), "acked A; B has not finished");

        let ev_b = status_event("2", "github-adapter", "done");
        let fired = review(&c, &board, &[ev_a, ev_b], &[ack], &registry);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].ticket(), "github-adapter");
    }

    #[test]
    fn scoped_ack_does_not_clear_other_subjects() {
        let registry = StateRegistry::builtin();
        let c = tkt(
            "slack-notify",
            &["deployment-record", "github-adapter"],
            watch_done_deps(),
        );
        let ev_a = status_event("1", "deployment-record", "done");
        let ev_b = status_event("2", "github-adapter", "done");
        let ack_b = Ack {
            id: "9".into(),
            by: "ren".into(),
            at: 1,
            against: Some("2".into()),
            hash: spec_hash(&c),
            scope: AckScope::Subject("github-adapter".into()),
            note: String::new(),
        };
        let fired = review(
            &c,
            std::slice::from_ref(&c),
            &[ev_a, ev_b],
            &[ack_b],
            &registry,
        );
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert_eq!(fired[0].ticket(), "deployment-record");
    }

    #[test]
    fn scoped_ack_does_not_clear_hash_fire() {
        let registry = StateRegistry::builtin();
        let mut c = tkt("slack-notify", &["a"], watch_done_deps());
        let unscoped = Ack {
            id: "0".into(),
            by: "ren".into(),
            at: 1,
            against: Some("1".into()),
            hash: spec_hash(&c),
            scope: AckScope::All,
            note: String::new(),
        };
        c.set_body("rewritten\n");
        let scoped = Ack {
            id: "1".into(),
            by: "ren".into(),
            at: 2,
            against: Some("9".into()),
            hash: spec_hash(&c),
            scope: AckScope::Subject("a".into()),
            note: String::new(),
        };
        let fired = review(&c, &[], &[], &[unscoped, scoped], &registry);
        assert!(
            fired.iter().any(|f| f.event().is_none()),
            "hash fire must survive a later subject-scoped ack: {fired:?}"
        );
    }

    #[test]
    fn hash_change_retriggers_without_new_event() {
        let registry = StateRegistry::builtin();
        let mut c = tkt("slack-notify", &["a"], watch_done_deps());
        let ack = Ack {
            id: "0".into(),
            by: "ren".into(),
            at: 1,
            against: Some("1".into()),
            hash: spec_hash(&c),
            scope: AckScope::All,
            note: String::new(),
        };
        c.set_body("rewritten\n");
        let fired = review(&c, &[], &[], &[ack], &registry);
        assert_eq!(fired.len(), 1);
        assert!(fired[0].event().is_none());
        assert_eq!(fired[0].reason(), HASH_CHANGED_REASON);
    }

    #[test]
    fn inbound_related_is_watched() {
        let registry = StateRegistry::builtin();
        let mut c = Ticket::new(
            "c",
            "C",
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
        c.set_watch(&[Watch::new("done", WatchOf::Related, "shared assumptions")])
            .unwrap();
        let peer = Ticket::new(
            "peer",
            "P",
            "done",
            Priority::P2,
            &[],
            &["c".into()],
            &[],
            &[],
            &[],
            &[],
            "",
        )
        .unwrap();
        let board = [c.clone(), peer];
        let ev = status_event("1", "peer", "done");
        let fired = review(&c, &board, &[ev], &[], &registry);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].ticket(), "peer");
    }

    #[test]
    fn claim_does_not_change_spec_hash() {
        let mut t = tkt("x", &[], watch_done_deps());
        let before = spec_hash(&t);
        t.set_claim("ren", 99, &StateRegistry::builtin()).unwrap();
        assert_eq!(spec_hash(&t), before);
    }

    #[test]
    fn ack_round_trips() {
        let a = Ack::new(AckSpec {
            by: "ren".into(),
            against: Some("1".into()),
            hash: "abc".into(),
            scope: AckScope::Subject("github-adapter".into()),
            note: "still valid".into(),
        });
        let again = Ack::parse(&a.render()).unwrap();
        assert_eq!(again.by, "ren");
        assert_eq!(again.against.as_deref(), Some("1"));
        assert_eq!(again.scope.as_ticket(), Some("github-adapter"));
        assert_eq!(again.note, "still valid");
        assert_eq!(again.hash, "abc");
    }
}
