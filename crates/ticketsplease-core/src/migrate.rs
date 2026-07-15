//! Frontmatter migration engine. Brings tickets up to the current schema by
//! applying ordered, round-trip-safe steps to each file.
//!
//! There is one step today — back-filling managed keys a hand-authored or older
//! ticket may be missing (status, priority, and the four list fields). Future
//! schema changes add steps here; each must edit through [`Document`] so unknown
//! keys, comments, and the body stay byte-for-byte intact.

use serde::Serialize;

use crate::error::{Error, Result};
use crate::frontmatter::Document;
use crate::store::{self, Store};

/// Summary of a migration run.
#[derive(Debug, Clone, Serialize)]
pub struct MigrateReport {
    /// Ids of tickets that were rewritten (sorted).
    pub migrated: Vec<String>,
    /// Count of tickets already current.
    pub unchanged: usize,
}

/// Migrate every ticket in the store. Files are rewritten atomically, and only when a
/// step actually changes them. With `dry_run`, nothing is written — the report still
/// lists the tickets that *would* be migrated, so callers can preview or detect drift.
pub fn migrate(store: &Store, dry_run: bool) -> Result<MigrateReport> {
    let mut migrated = Vec::new();
    let mut unchanged = 0;
    for path in store.ticket_files()? {
        let raw = std::fs::read_to_string(&path).map_err(Error::Io)?;
        let mut doc = Document::parse(&raw)?;
        let before = doc.render();
        backfill_managed_keys(&mut doc)?;
        let after = doc.render();
        if after != before {
            if !dry_run {
                store::write_atomic(&path, &after)?;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            migrated.push(id);
        } else {
            unchanged += 1;
        }
    }
    migrated.sort();
    Ok(MigrateReport {
        migrated,
        unchanged,
    })
}

/// Step 1 → schema v1: ensure every managed key is present.
fn backfill_managed_keys(doc: &mut Document) -> Result<()> {
    if !doc.has_key("status") {
        doc.set_scalar("status", "todo")?;
    }
    if !doc.has_key("priority") {
        doc.set_scalar("priority", "p2")?;
    }
    for key in ["dependencies", "scopes", "paths", "tags"] {
        doc.ensure_empty_list(key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backfill_adds_missing_keys_only() {
        // A minimal ticket missing status/priority/lists, plus a custom key.
        let raw = "---\nid: x\ntitle: T\ncustom: keep\n---\nbody\n";
        let mut doc = Document::parse(raw).unwrap();
        backfill_managed_keys(&mut doc).unwrap();
        let out = doc.render();
        assert!(out.contains("status: todo\n"));
        assert!(out.contains("priority: p2\n"));
        assert!(out.contains("dependencies: []\n"));
        assert!(out.contains("scopes: []\n"));
        assert!(out.contains("custom: keep\n")); // untouched
        assert!(out.contains("\n---\nbody\n")); // body untouched

        // Idempotent: a second pass changes nothing.
        let before = doc.render();
        backfill_managed_keys(&mut doc).unwrap();
        assert_eq!(doc.render(), before);
    }
}
