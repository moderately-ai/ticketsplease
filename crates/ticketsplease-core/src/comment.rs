//! Per-ticket comments: append-only markdown files under
//! `<tickets_dir>/<id>.comments/`, one file per comment.
//!
//! One file per comment is the whole trick: distinct, time-sortable filenames
//! mean two concurrent authors never touch the same path — so there is neither a
//! filesystem write race (single shared working tree) nor a git merge conflict
//! (separate per-branch worktrees folded into `main`). No lock, no merge driver.
//! A comment file is itself a frontmatter document, so it round-trips and is
//! hand-editable like a ticket.

use yaml_rust2::YamlLoader;

use crate::error::{Error, Result};
use crate::frontmatter::Document;
use crate::ids;

/// A single comment on a ticket.
#[derive(Debug, Clone)]
pub struct Comment {
    /// Sortable unique id `<epoch_millis>-<rand>`; the prefix orders chronologically.
    pub id: String,
    /// Author, when attributed via `--as`.
    pub by: Option<String>,
    /// Creation time (epoch seconds).
    pub at: Option<u64>,
    /// Id of the comment this replies to, for threading.
    pub reply_to: Option<String>,
    /// Markdown body.
    pub body: String,
}

impl Comment {
    /// Build a new comment with a freshly generated id and current timestamp.
    #[must_use]
    pub fn new(by: Option<String>, reply_to: Option<String>, body: &str) -> Self {
        Self {
            id: ids::new_id(),
            by,
            at: Some(ids::now_secs()),
            reply_to,
            // Canonicalize: a trailing newline is insignificant, and dropping it
            // keeps `new` / `parse` / `render` byte-consistent.
            body: body.trim_end_matches('\n').to_string(),
        }
    }

    /// Parse a comment file (frontmatter + markdown body).
    pub fn parse(raw: &str) -> Result<Self> {
        let doc = Document::parse(raw)?;
        let docs = YamlLoader::load_from_str(doc.fm())
            .map_err(|e| Error::Invalid(format!("invalid comment frontmatter: {e}")))?;
        let y = docs
            .first()
            .ok_or_else(|| Error::Invalid("empty comment frontmatter".into()))?;
        let id = y["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("comment missing `id`".into()))?;
        let at = y["at"]
            .as_i64()
            .map(|n| n as u64)
            .or_else(|| y["at"].as_str().and_then(|s| s.parse().ok()));
        Ok(Self {
            id,
            by: y["by"].as_str().map(str::to_string),
            at,
            reply_to: y["reply_to"].as_str().map(str::to_string),
            body: doc.body().trim_end_matches('\n').to_string(),
        })
    }

    /// Serialize to the on-disk file form (frontmatter + body).
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::from("---\n");
        s.push_str(&format!("id: {}\n", self.id));
        if let Some(by) = &self.by {
            s.push_str(&format!("by: {}\n", yaml_dq(by)));
        }
        if let Some(at) = self.at {
            s.push_str(&format!("at: {at}\n"));
        }
        if let Some(rt) = &self.reply_to {
            s.push_str(&format!("reply_to: {rt}\n"));
        }
        s.push_str("---\n");
        s.push_str(self.body.trim_end_matches('\n'));
        s.push('\n');
        s
    }
}

/// Double-quote a scalar so arbitrary author strings (e.g. `@worker`, `a: b`) stay
/// valid YAML. Escapes the two chars that matter inside a double-quoted scalar.
fn yaml_dq(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_render_and_parse() {
        let c = Comment::new(Some("worker-1".into()), None, "a note\nwith two lines");
        let parsed = Comment::parse(&c.render()).unwrap();
        assert_eq!(parsed.id, c.id);
        assert_eq!(parsed.by.as_deref(), Some("worker-1"));
        assert_eq!(parsed.body, "a note\nwith two lines");
        assert!(parsed.at.is_some());
    }

    #[test]
    fn ids_are_time_sortable_and_unique() {
        let a = Comment::new(None, None, "x");
        let b = Comment::new(None, None, "y");
        assert_ne!(a.id, b.id, "ids must be unique");
        // The (non-decreasing) timestamp prefix keeps the lexical sort chronological.
        assert!(b.id[..19] >= a.id[..19]);
    }

    #[test]
    fn quotes_author_with_yaml_metacharacters() {
        let c = Comment::new(
            Some("@orchestrator: lead".into()),
            Some("123-abc".into()),
            "hi",
        );
        let parsed = Comment::parse(&c.render()).unwrap();
        assert_eq!(parsed.by.as_deref(), Some("@orchestrator: lead"));
        assert_eq!(parsed.reply_to.as_deref(), Some("123-abc"));
    }
}
