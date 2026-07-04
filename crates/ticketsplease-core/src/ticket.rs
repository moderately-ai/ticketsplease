//! The ticket data model and typed reads over a [`Document`].
//!
//! Reads use a real YAML parser for correctness; all mutations go through the
//! round-trip-safe [`Document`] so writes stay line-surgical.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use yaml_rust2::{Yaml, YamlLoader};

use crate::error::{Error, Result};
use crate::frontmatter::Document;

/// Ticket lifecycle status. Ordering follows declaration order and is not
/// otherwise meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Newly created, not yet started.
    #[default]
    Todo,
    /// Author-flagged as ready to pick up.
    Ready,
    /// Actively being worked.
    InProgress,
    /// Cannot proceed.
    Blocked,
    /// Awaiting review.
    Review,
    /// Completed successfully.
    Done,
    /// Terminated without completion — won't-do, duplicate, obsolete, superseded,
    /// cancelled. Terminal like `done` (excluded from scheduling, drops its claim) but
    /// deliberately does *not* satisfy dependents; see [`Status::completes_dependencies`].
    Closed,
}

impl Status {
    /// The canonical lowercase/kebab string written to frontmatter.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Todo => "todo",
            Status::Ready => "ready",
            Status::InProgress => "in-progress",
            Status::Blocked => "blocked",
            Status::Review => "review",
            Status::Done => "done",
            Status::Closed => "closed",
        }
    }

    /// Whether a ticket in this status is eligible for dispatch (todo or ready).
    #[must_use]
    pub fn is_dispatchable(self) -> bool {
        matches!(self, Status::Todo | Status::Ready)
    }

    /// Whether a ticket in this status is actively open (in-progress or review),
    /// which the guard treats as occupying its declared scopes.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Status::InProgress | Status::Review)
    }

    /// Whether this is a terminal status (finished for scheduling): `done` or `closed`.
    /// Terminal tickets are excluded from the ready/dispatch queue and drop their claim,
    /// but only `done` also [`completes_dependencies`](Self::completes_dependencies).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Closed)
    }

    /// Whether reaching this status satisfies a dependent's dependency — `done` only.
    /// A `closed` (abandoned) prerequisite is terminal but deliberately does *not*
    /// unblock its dependents: they are surfaced as orphaned rather than silently
    /// dispatched onto dropped work or silently deadlocked.
    #[must_use]
    pub fn completes_dependencies(self) -> bool {
        matches!(self, Status::Done)
    }
}

impl FromStr for Status {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "todo" => Status::Todo,
            "ready" => Status::Ready,
            "in-progress" => Status::InProgress,
            "blocked" => Status::Blocked,
            "review" => Status::Review,
            "done" => Status::Done,
            "closed" => Status::Closed,
            _ => {
                return Err(Error::Invalid(format!(
                "unknown status `{s}` (expected todo|ready|in-progress|blocked|review|done|closed)"
            )))
            }
        })
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a ticket was `closed` (terminated without completion). A deliberately small,
/// fixed vocabulary — closed like GitHub's handful of close reasons, not Jira's
/// sprawling customizable list — so it stays queryable and stable. A ticket may be
/// closed with no reason at all; this is only recorded when one is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClosedReason {
    /// A duplicate of another ticket.
    Duplicate,
    /// Decided against — will not be done.
    WontDo,
    /// No longer relevant.
    Obsolete,
    /// Replaced by a different ticket or approach.
    Superseded,
    /// Cancelled for another reason.
    Cancelled,
}

impl ClosedReason {
    /// The canonical lowercase string written to frontmatter.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedReason::Duplicate => "duplicate",
            ClosedReason::WontDo => "wontdo",
            ClosedReason::Obsolete => "obsolete",
            ClosedReason::Superseded => "superseded",
            ClosedReason::Cancelled => "cancelled",
        }
    }
}

impl FromStr for ClosedReason {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "duplicate" => ClosedReason::Duplicate,
            "wontdo" => ClosedReason::WontDo,
            "obsolete" => ClosedReason::Obsolete,
            "superseded" => ClosedReason::Superseded,
            "cancelled" => ClosedReason::Cancelled,
            _ => {
                return Err(Error::Invalid(format!(
                    "unknown close reason `{s}` (expected duplicate|wontdo|obsolete|superseded|cancelled)"
                )))
            }
        })
    }
}

impl std::fmt::Display for ClosedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Ticket priority. `P0` is highest; declaration order gives `P0 < P1 < P2 < P3`
/// so an ascending sort puts the highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Highest.
    P0,
    /// High.
    P1,
    /// Normal (default).
    #[default]
    P2,
    /// Low.
    P3,
}

impl Priority {
    /// The canonical lowercase string written to frontmatter.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::P0 => "p0",
            Priority::P1 => "p1",
            Priority::P2 => "p2",
            Priority::P3 => "p3",
        }
    }
}

impl FromStr for Priority {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "p0" => Priority::P0,
            "p1" => Priority::P1,
            "p2" => Priority::P2,
            "p3" => Priority::P3,
            _ => {
                return Err(Error::Invalid(format!(
                    "unknown priority `{s}` (expected p0..p3)"
                )))
            }
        })
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single ticket: typed fields for queries plus the underlying [`Document`]
/// that backs round-trip-safe writes.
#[derive(Debug, Clone)]
pub struct Ticket {
    /// Stable slug identifier; equals the file stem.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Lifecycle status.
    pub status: Status,
    /// Priority.
    pub priority: Priority,
    /// IDs of tickets this one depends on.
    pub dependencies: Vec<String>,
    /// IDs of thematically-related tickets. A soft, non-blocking cross-reference:
    /// recorded structurally (queryable, graphable) but ignored by readiness,
    /// `tracks`, and cycle detection — unlike `dependencies`, which gate scheduling.
    pub related: Vec<String>,
    /// Abstract scope names this ticket claims *exclusively* (a rewrite): two tickets
    /// that both claim a scope here cannot run in parallel.
    pub scopes: Vec<String>,
    /// Scope names this ticket claims in *shared* (additive) mode — it only appends to
    /// or extends that area. Two tickets that both hold a scope shared are compatible
    /// (may run in parallel); a shared claim still conflicts with an exclusive one.
    pub shared_scopes: Vec<String>,
    /// Explicit path globs (augment `scopes`).
    pub paths: Vec<String>,
    /// Free-form tags.
    pub tags: Vec<String>,
    /// Agent currently holding the claim, if any.
    pub assignee: Option<String>,
    /// Claim lease expiry (epoch seconds), if claimed.
    pub lease_expires_at: Option<u64>,
    /// The status the ticket held before it was claimed, so `release` can restore it
    /// instead of unconditionally landing in `ready`. Present only while claimed.
    pub claimed_from: Option<Status>,
    /// Why the ticket was closed, when `status: closed` and a reason was recorded.
    /// Only meaningful while `closed`; cleared on any transition away.
    pub closed_reason: Option<ClosedReason>,
    /// A free-text one-line note accompanying a close. Cleared with `closed_reason`.
    pub closed_note: Option<String>,
    doc: Document,
    /// The file this ticket was loaded from, if any. `save` writes back here so a
    /// ticket whose frontmatter `id` has drifted from its filename updates in place
    /// rather than spawning a fresh `<id>.md` (which would orphan the original and
    /// create a duplicate id). `None` for tickets built in memory or parsed from a
    /// git ref — those are never the target of a disk write-back.
    source_path: Option<PathBuf>,
}

impl Ticket {
    /// Parse a ticket from raw file contents.
    pub fn parse(raw: &str) -> Result<Self> {
        let doc = Document::parse(raw)?;
        let docs = YamlLoader::load_from_str(doc.fm())
            .map_err(|e| Error::Invalid(format!("invalid YAML frontmatter: {e}")))?;
        let y = docs
            .first()
            .ok_or_else(|| Error::Invalid("empty frontmatter".into()))?;

        let id = require_string(y, "id")?;
        let title = require_string(y, "title")?;
        let status = optional_string(y, "status")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or_default();
        let priority = optional_string(y, "priority")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            id,
            title,
            status,
            priority,
            dependencies: string_list(y, "dependencies"),
            related: string_list(y, "related"),
            scopes: string_list(y, "scopes"),
            shared_scopes: string_list(y, "shared_scopes"),
            paths: string_list(y, "paths"),
            tags: string_list(y, "tags"),
            assignee: optional_string(y, "assignee"),
            lease_expires_at: optional_string(y, "lease_expires_at").and_then(|s| s.parse().ok()),
            claimed_from: optional_string(y, "claimed_from").and_then(|s| s.parse().ok()),
            closed_reason: optional_string(y, "closed_reason")
                .map(|s| s.parse())
                .transpose()?,
            closed_note: optional_string(y, "closed_note"),
            doc,
            source_path: None,
        })
    }

    /// Load and parse a ticket from a file, remembering the path for write-back.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", path.display())))?;
        let mut t = Self::parse(&raw)?;
        t.source_path = Some(path.to_path_buf());
        Ok(t)
    }

    /// The file this ticket was loaded from (`None` if built in memory or parsed
    /// from a string/ref). `Store::save` writes back here to keep edits in place.
    #[must_use]
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// The markdown body.
    #[must_use]
    pub fn body(&self) -> &str {
        self.doc.body()
    }

    /// Serialize the ticket back to its exact file representation.
    #[must_use]
    pub fn render(&self) -> String {
        self.doc.render()
    }

    /// Set the status (surgical write). Any transition *away* from `closed` clears the
    /// resolution metadata (`closed_reason`/`closed_note`) in the same write, so the
    /// live frontmatter never carries a stale "why" contradicting the current status —
    /// this is what makes `reopen` atomic and self-cleaning.
    pub fn set_status(&mut self, status: Status) -> Result<()> {
        self.doc.set_scalar("status", status.as_str())?;
        if status != Status::Closed {
            self.clear_closed_meta();
        }
        self.status = status;
        Ok(())
    }

    /// Record why a `closed` ticket was closed (surgical). The caller is responsible for
    /// ensuring the status is `closed` — a reason on a non-closed ticket is meaningless.
    pub fn set_closed_reason(&mut self, reason: ClosedReason) -> Result<()> {
        self.doc.set_scalar("closed_reason", reason.as_str())?;
        self.closed_reason = Some(reason);
        Ok(())
    }

    /// Attach a free-text one-line close note (surgical).
    pub fn set_closed_note(&mut self, note: &str) -> Result<()> {
        self.doc.set_scalar("closed_note", note)?;
        self.closed_note = Some(note.to_string());
        Ok(())
    }

    /// Drop the close reason + note. Called on any transition out of `closed` (see
    /// [`set_status`](Self::set_status)); idempotent when neither is present.
    pub fn clear_closed_meta(&mut self) {
        self.doc.remove_key("closed_reason");
        self.doc.remove_key("closed_note");
        self.closed_reason = None;
        self.closed_note = None;
    }

    /// Set the priority (surgical write).
    pub fn set_priority(&mut self, priority: Priority) -> Result<()> {
        self.doc.set_scalar("priority", priority.as_str())?;
        self.priority = priority;
        Ok(())
    }

    /// Set the id (surgical write). Used by `rename`; the caller is responsible for
    /// moving the file to match the new id.
    pub fn set_id(&mut self, id: &str) -> Result<()> {
        self.doc.set_scalar("id", id)?;
        self.id = id.to_string();
        Ok(())
    }

    /// Set the title (surgical write).
    pub fn set_title(&mut self, title: &str) -> Result<()> {
        self.doc.set_scalar("title", title)?;
        self.title = title.to_string();
        Ok(())
    }

    /// Add an explicit path glob (idempotent). Returns whether anything changed.
    pub fn add_path(&mut self, path: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("paths", path)?;
        if changed {
            self.paths.push(path.to_string());
        }
        Ok(changed)
    }

    /// Remove an explicit path glob (idempotent). Returns whether anything changed.
    pub fn remove_path(&mut self, path: &str) -> Result<bool> {
        let changed = self.doc.remove_list_item("paths", path)?;
        if changed {
            self.paths.retain(|p| p != path);
        }
        Ok(changed)
    }

    /// Add a dependency (idempotent). Returns whether anything changed.
    pub fn add_dependency(&mut self, id: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("dependencies", id)?;
        if changed {
            self.dependencies.push(id.to_string());
        }
        Ok(changed)
    }

    /// Remove a dependency (idempotent). Returns whether anything changed.
    pub fn remove_dependency(&mut self, id: &str) -> Result<bool> {
        let changed = self.doc.remove_list_item("dependencies", id)?;
        if changed {
            self.dependencies.retain(|d| d != id);
        }
        Ok(changed)
    }

    /// Add a non-blocking related link (idempotent). Returns whether anything
    /// changed. Unlike a dependency, this is never cycle-checked — related links
    /// carry no ordering, so a cycle among them is harmless.
    pub fn add_related(&mut self, id: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("related", id)?;
        if changed {
            self.related.push(id.to_string());
        }
        Ok(changed)
    }

    /// Remove a related link (idempotent). Returns whether anything changed.
    pub fn remove_related(&mut self, id: &str) -> Result<bool> {
        let changed = self.doc.remove_list_item("related", id)?;
        if changed {
            self.related.retain(|r| r != id);
        }
        Ok(changed)
    }

    /// Add a scope (idempotent). Returns whether anything changed.
    pub fn add_scope(&mut self, scope: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("scopes", scope)?;
        if changed {
            self.scopes.push(scope.to_string());
        }
        Ok(changed)
    }

    /// Remove a scope (idempotent). Returns whether anything changed.
    pub fn remove_scope(&mut self, scope: &str) -> Result<bool> {
        let changed = self.doc.remove_list_item("scopes", scope)?;
        if changed {
            self.scopes.retain(|s| s != scope);
        }
        Ok(changed)
    }

    /// Add a shared (additive) scope claim (idempotent). Returns whether changed.
    pub fn add_shared_scope(&mut self, scope: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("shared_scopes", scope)?;
        if changed {
            self.shared_scopes.push(scope.to_string());
        }
        Ok(changed)
    }

    /// Remove a shared scope claim (idempotent). Returns whether changed.
    pub fn remove_shared_scope(&mut self, scope: &str) -> Result<bool> {
        let changed = self.doc.remove_list_item("shared_scopes", scope)?;
        if changed {
            self.shared_scopes.retain(|s| s != scope);
        }
        Ok(changed)
    }

    /// Add a tag (idempotent). Returns whether anything changed.
    pub fn add_tag(&mut self, tag: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("tags", tag)?;
        if changed {
            self.tags.push(tag.to_string());
        }
        Ok(changed)
    }

    /// Remove a tag (idempotent). Returns whether anything changed.
    pub fn remove_tag(&mut self, tag: &str) -> Result<bool> {
        let changed = self.doc.remove_list_item("tags", tag)?;
        if changed {
            self.tags.retain(|t| t != tag);
        }
        Ok(changed)
    }

    /// Record a claim: set status in-progress, assignee, and the lease expiry.
    pub fn set_claim(&mut self, assignee: &str, lease_expires_at: u64) -> Result<()> {
        // Remember the pre-claim status so `release` can restore it. Don't overwrite
        // it when renewing a claim that is already in-progress.
        if self.status != Status::InProgress {
            self.doc.set_scalar("claimed_from", self.status.as_str())?;
            self.claimed_from = Some(self.status);
        }
        self.set_status(Status::InProgress)?;
        self.doc.set_scalar("assignee", assignee)?;
        // Write the lease as a bare integer so frontmatter matches the JSON type.
        self.doc
            .set_scalar_raw("lease_expires_at", &lease_expires_at.to_string())?;
        self.assignee = Some(assignee.to_string());
        self.lease_expires_at = Some(lease_expires_at);
        Ok(())
    }

    /// Clear a claim. If the ticket is still `in-progress` (the claim never advanced),
    /// restore the pre-claim status (`claimed_from`, default `ready`); if the worker
    /// already moved it on (review/blocked/done), keep that — releasing must not
    /// revert real progress.
    pub fn clear_claim(&mut self) -> Result<()> {
        if self.status == Status::InProgress {
            let restore = self.claimed_from.unwrap_or(Status::Ready);
            self.set_status(restore)?;
        }
        self.clear_lease();
        Ok(())
    }

    /// Drop the claim fields (assignee, lease, pre-claim marker) without touching the
    /// status. Used when a ticket reaches a terminal status — completion ends the
    /// claim, but must keep the `done` status rather than reverting to `ready`.
    pub fn clear_lease(&mut self) {
        self.doc.remove_key("assignee");
        self.doc.remove_key("lease_expires_at");
        self.doc.remove_key("claimed_from");
        self.assignee = None;
        self.lease_expires_at = None;
        self.claimed_from = None;
    }

    /// Whether this ticket's claim lease is still live at `now` (epoch seconds).
    #[must_use]
    pub fn lease_live(&self, now: u64) -> bool {
        self.lease_expires_at.is_some_and(|exp| exp > now)
    }

    /// Replace the markdown body (frontmatter stays byte-for-byte intact).
    pub fn set_body(&mut self, body: &str) {
        self.doc.set_body(body);
    }

    /// Append text to the markdown body.
    pub fn append_body(&mut self, text: &str) {
        self.doc.append_body(text);
    }

    /// Construct a new ticket from fields, rendering canonical frontmatter.
    #[allow(clippy::too_many_arguments)] // a flat ticket; a builder adds no clarity
    pub fn new(
        id: &str,
        title: &str,
        status: Status,
        priority: Priority,
        dependencies: &[String],
        related: &[String],
        scopes: &[String],
        shared_scopes: &[String],
        paths: &[String],
        tags: &[String],
        body: &str,
    ) -> Result<Self> {
        use crate::frontmatter::{render_inline_list, render_scalar};
        let mut s = String::new();
        s.push_str("---\n");
        s.push_str(&format!("id: {}\n", render_scalar(id)));
        s.push_str(&format!("title: {}\n", render_scalar(title)));
        s.push_str(&format!("status: {}\n", status.as_str()));
        s.push_str(&format!("priority: {}\n", priority.as_str()));
        s.push_str(&format!(
            "dependencies: {}\n",
            render_inline_list(dependencies)
        ));
        s.push_str(&format!("related: {}\n", render_inline_list(related)));
        s.push_str(&format!("scopes: {}\n", render_inline_list(scopes)));
        s.push_str(&format!(
            "shared_scopes: {}\n",
            render_inline_list(shared_scopes)
        ));
        s.push_str(&format!("paths: {}\n", render_inline_list(paths)));
        s.push_str(&format!("tags: {}\n", render_inline_list(tags)));
        s.push_str("---\n");
        s.push_str(body);
        if !body.is_empty() && !body.ends_with('\n') {
            s.push('\n');
        }
        Self::parse(&s)
    }
}

fn require_string(y: &Yaml, key: &str) -> Result<String> {
    scalar_to_string(&y[key])
        .ok_or_else(|| Error::Invalid(format!("frontmatter missing required `{key}`")))
}

fn optional_string(y: &Yaml, key: &str) -> Option<String> {
    scalar_to_string(&y[key])
}

fn string_list(y: &Yaml, key: &str) -> Vec<String> {
    match y[key].as_vec() {
        Some(v) => v.iter().filter_map(scalar_to_string).collect(),
        None => Vec::new(),
    }
}

fn scalar_to_string(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Real(r) => Some(r.clone()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nid: foo\ntitle: A title\nstatus: review\npriority: p1\ndependencies: [a, b]\nscopes:\n  - one\n---\nBody.\n";

    #[test]
    fn parses_typed_fields() {
        let t = Ticket::parse(SAMPLE).unwrap();
        assert_eq!(t.id, "foo");
        assert_eq!(t.title, "A title");
        assert_eq!(t.status, Status::Review);
        assert_eq!(t.priority, Priority::P1);
        assert_eq!(t.dependencies, vec!["a", "b"]);
        assert_eq!(t.scopes, vec!["one"]);
        assert!(t.status.is_open());
    }

    #[test]
    fn missing_status_priority_default() {
        let raw = "---\nid: x\ntitle: T\n---\n";
        let t = Ticket::parse(raw).unwrap();
        assert_eq!(t.status, Status::Todo);
        assert_eq!(t.priority, Priority::P2);
    }

    #[test]
    fn missing_required_id_errors() {
        let raw = "---\ntitle: T\n---\n";
        assert!(Ticket::parse(raw).is_err());
    }

    #[test]
    fn mutation_round_trips_through_document() {
        let mut t = Ticket::parse(SAMPLE).unwrap();
        t.set_status(Status::Done).unwrap();
        assert!(t.add_dependency("c").unwrap());
        let out = t.render();
        assert!(out.contains("status: done\n"));
        assert!(out.contains("dependencies: [a, b, c]\n"));
        // Reparse to confirm the written form is valid and consistent.
        let again = Ticket::parse(&out).unwrap();
        assert_eq!(again.status, Status::Done);
        assert_eq!(again.dependencies, vec!["a", "b", "c"]);
    }

    #[test]
    fn related_is_a_separate_list_from_dependencies() {
        let raw = "---\nid: foo\ntitle: T\ndependencies: [a]\nrelated: [b, c]\n---\n";
        let t = Ticket::parse(raw).unwrap();
        assert_eq!(t.dependencies, vec!["a"]);
        assert_eq!(t.related, vec!["b", "c"]);
    }

    #[test]
    fn new_renders_related_inline_and_round_trips() {
        let t = Ticket::new(
            "foo",
            "T",
            Status::Todo,
            Priority::P2,
            &["a".into()],
            &["b".into()],
            &[],
            &["sh".into()],
            &[],
            &[],
            "",
        )
        .unwrap();
        let out = t.render();
        assert!(out.contains("dependencies: [a]\n"));
        assert!(out.contains("related: [b]\n"));
        assert!(out.contains("shared_scopes: [sh]\n"));
        // Mutators stay inline because the key already exists from `new`.
        let mut t = Ticket::parse(&out).unwrap();
        assert!(t.add_related("c").unwrap());
        assert!(!t.add_related("c").unwrap(), "idempotent");
        assert!(t.render().contains("related: [b, c]\n"));
        assert!(t.remove_related("b").unwrap());
        assert_eq!(Ticket::parse(&t.render()).unwrap().related, vec!["c"]);
    }

    #[test]
    fn status_and_priority_parse_case_insensitively() {
        assert_eq!("TODO".parse::<Status>().unwrap(), Status::Todo);
        assert_eq!(
            " In-Progress ".parse::<Status>().unwrap(),
            Status::InProgress
        );
        assert_eq!("P0".parse::<Priority>().unwrap(), Priority::P0);
        let err = "doing".parse::<Status>().unwrap_err().to_string();
        assert!(
            err.contains("expected todo|ready"),
            "lists valid values: {err}"
        );
    }

    #[test]
    fn closed_is_terminal_but_does_not_complete_dependencies() {
        assert_eq!("closed".parse::<Status>().unwrap(), Status::Closed);
        assert_eq!(Status::Closed.as_str(), "closed");
        assert!(Status::Closed.is_terminal());
        assert!(Status::Done.is_terminal());
        // The whole point: closed is terminal but does NOT satisfy dependents.
        assert!(!Status::Closed.completes_dependencies());
        assert!(Status::Done.completes_dependencies());
        assert!(!Status::Closed.is_dispatchable());
        assert!(!Status::Closed.is_open());
    }

    #[test]
    fn closed_reason_round_trips_and_reopen_clears_it() {
        let mut t = Ticket::parse(SAMPLE).unwrap();
        t.set_status(Status::Closed).unwrap();
        t.set_closed_reason(ClosedReason::WontDo).unwrap();
        t.set_closed_note("superseded by a new approach").unwrap();
        let out = t.render();
        assert!(out.contains("status: closed\n"));
        assert!(out.contains("closed_reason: wontdo\n"));
        assert!(out.contains("closed_note:"));
        // The written form reparses to the same typed values.
        let again = Ticket::parse(&out).unwrap();
        assert_eq!(again.status, Status::Closed);
        assert_eq!(again.closed_reason, Some(ClosedReason::WontDo));
        assert_eq!(
            again.closed_note.as_deref(),
            Some("superseded by a new approach")
        );

        // Reopening (any transition away from closed) clears the resolution atomically.
        let mut reopened = again;
        reopened.set_status(Status::Todo).unwrap();
        assert_eq!(reopened.status, Status::Todo);
        assert!(reopened.closed_reason.is_none());
        assert!(reopened.closed_note.is_none());
        let out2 = reopened.render();
        assert!(!out2.contains("closed_reason"));
        assert!(!out2.contains("closed_note"));
    }

    #[test]
    fn close_reason_parses_case_insensitively_and_rejects_typos() {
        assert_eq!(
            "WontDo".parse::<ClosedReason>().unwrap(),
            ClosedReason::WontDo
        );
        assert_eq!(
            " duplicate ".parse::<ClosedReason>().unwrap(),
            ClosedReason::Duplicate
        );
        let err = "nope".parse::<ClosedReason>().unwrap_err().to_string();
        assert!(err.contains("expected duplicate|wontdo"), "{err}");
    }

    #[test]
    fn priority_orders_p0_first() {
        let mut ps = [Priority::P3, Priority::P0, Priority::P2, Priority::P1];
        ps.sort();
        assert_eq!(ps, [Priority::P0, Priority::P1, Priority::P2, Priority::P3]);
    }
}
