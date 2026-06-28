//! Saved named filter views — reusable `--where` expressions ("the epic view").
//!
//! Views live in `<repo>/.ticketsplease/views.toml`, a tool-owned file separate from
//! the hand-authored `ticketsplease.toml` (which is read-only to the tool, so we never
//! risk clobbering its comments). The file is meant to be committed — a saved view is
//! a shared project artifact, not local state. Each stored expression is validated
//! with [`crate::query::parse`] at save time, so a malformed view fails when it is
//! written rather than when it is later used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::write_atomic;

/// The tool-owned state directory at the repo root.
pub const STATE_DIR: &str = ".ticketsplease";
/// The saved-views file within [`STATE_DIR`].
pub const VIEWS_FILE: &str = "views.toml";

/// A single saved view: a stored `--where` expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The boolean filter expression (see [`crate::query`]).
    #[serde(rename = "where")]
    pub where_expr: String,
}

/// The parsed `views.toml`: name -> view (sorted for deterministic output).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Views {
    #[serde(default)]
    views: BTreeMap<String, View>,
}

impl Views {
    /// Load `<repo>/.ticketsplease/views.toml`. A missing file is an empty set (not
    /// an error) — saved views are optional.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = Self::path(repo_root);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Invalid(format!("invalid {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Write the views back to `<repo>/.ticketsplease/views.toml` (atomically),
    /// creating the state directory if needed.
    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let dir = repo_root.join(STATE_DIR);
        std::fs::create_dir_all(&dir).map_err(Error::Io)?;
        let text = toml::to_string(self)
            .map_err(|e| Error::Internal(format!("serializing views: {e}")))?;
        write_atomic(&dir.join(VIEWS_FILE), &text)
    }

    /// Path to the views file under a repo root.
    #[must_use]
    pub fn path(repo_root: &Path) -> PathBuf {
        repo_root.join(STATE_DIR).join(VIEWS_FILE)
    }

    /// Look up a view by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&View> {
        self.views.get(name)
    }

    /// All views, sorted by name.
    #[must_use]
    pub fn all(&self) -> &BTreeMap<String, View> {
        &self.views
    }

    /// Add or replace a view, validating that its expression parses. Returns whether
    /// an existing view of the same name was replaced.
    pub fn insert(&mut self, name: &str, where_expr: &str) -> Result<bool> {
        if name.is_empty() {
            return Err(Error::Invalid("view name must not be empty".into()));
        }
        // Validate eagerly so a bad expression is rejected at save, not at use.
        crate::query::parse(where_expr)?;
        let replaced = self
            .views
            .insert(
                name.to_string(),
                View {
                    where_expr: where_expr.to_string(),
                },
            )
            .is_some();
        Ok(replaced)
    }

    /// Remove a view. Returns whether it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.views.remove(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_validates_and_round_trips_through_toml() {
        let mut v = Views::default();
        assert!(!v.insert("epic", "tag:dialect AND NOT status:done").unwrap());
        assert!(v.insert("epic", "tag:dialect").unwrap(), "replace reported");
        assert_eq!(v.get("epic").unwrap().where_expr, "tag:dialect");

        let text = toml::to_string(&v).unwrap();
        assert!(text.contains("[views.epic]"));
        assert!(text.contains("where = \"tag:dialect\""));
        let back: Views = toml::from_str(&text).unwrap();
        assert_eq!(back.get("epic"), v.get("epic"));
    }

    #[test]
    fn insert_rejects_a_malformed_expression() {
        let mut v = Views::default();
        assert!(v.insert("bad", "bogus:x").is_err());
        assert!(v.insert("bad", "(tag:x").is_err());
        assert!(v.insert("", "tag:x").is_err());
        assert!(v.get("bad").is_none());
    }

    #[test]
    fn remove_reports_existence() {
        let mut v = Views::default();
        v.insert("a", "tag:x").unwrap();
        assert!(v.remove("a"));
        assert!(!v.remove("a"));
    }
}
