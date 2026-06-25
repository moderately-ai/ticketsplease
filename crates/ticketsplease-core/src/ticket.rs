//! The ticket data model and typed reads over a [`Document`].
//!
//! Reads use a real YAML parser for correctness; all mutations go through the
//! round-trip-safe [`Document`] so writes stay line-surgical.

use std::path::Path;
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
    /// Complete.
    Done,
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
}

impl FromStr for Status {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "todo" => Status::Todo,
            "ready" => Status::Ready,
            "in-progress" => Status::InProgress,
            "blocked" => Status::Blocked,
            "review" => Status::Review,
            "done" => Status::Done,
            other => return Err(Error::Invalid(format!("unknown status `{other}`"))),
        })
    }
}

impl std::fmt::Display for Status {
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
        Ok(match s {
            "p0" => Priority::P0,
            "p1" => Priority::P1,
            "p2" => Priority::P2,
            "p3" => Priority::P3,
            other => {
                return Err(Error::Invalid(format!(
                    "unknown priority `{other}` (expected p0..p3)"
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
    /// Abstract scope names this ticket declares.
    pub scopes: Vec<String>,
    /// Explicit path globs (augment `scopes`).
    pub paths: Vec<String>,
    /// Free-form tags.
    pub tags: Vec<String>,
    doc: Document,
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
            scopes: string_list(y, "scopes"),
            paths: string_list(y, "paths"),
            tags: string_list(y, "tags"),
            doc,
        })
    }

    /// Load and parse a ticket from a file.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&raw)
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

    /// Set the status (surgical write).
    pub fn set_status(&mut self, status: Status) -> Result<()> {
        self.doc.set_scalar("status", status.as_str())?;
        self.status = status;
        Ok(())
    }

    /// Set the priority (surgical write).
    pub fn set_priority(&mut self, priority: Priority) -> Result<()> {
        self.doc.set_scalar("priority", priority.as_str())?;
        self.priority = priority;
        Ok(())
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

    /// Add a tag (idempotent). Returns whether anything changed.
    pub fn add_tag(&mut self, tag: &str) -> Result<bool> {
        let changed = self.doc.add_list_item("tags", tag)?;
        if changed {
            self.tags.push(tag.to_string());
        }
        Ok(changed)
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
        scopes: &[String],
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
        s.push_str(&format!("scopes: {}\n", render_inline_list(scopes)));
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
    fn priority_orders_p0_first() {
        let mut ps = [Priority::P3, Priority::P0, Priority::P2, Priority::P1];
        ps.sort();
        assert_eq!(ps, [Priority::P0, Priority::P1, Priority::P2, Priority::P3]);
    }
}
