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
use crate::store::Store;
use crate::txn::plan_upserts;

/// Managed frontmatter keys the schema guarantees on every ticket. A ticket missing any
/// of these is "behind" and [`backfill_managed_keys`] adds it. Kept as one list so the
/// drift predicate ([`needs_backfill`]) and the backfill step can never disagree.
///
/// `status`/`priority` are scalars, the rest are lists; both back-fill idempotently
/// (each writes only when its key is absent), so key presence alone decides drift.
const MANAGED_KEYS: [&str; 6] = [
    "status",
    "priority",
    "dependencies",
    "scopes",
    "paths",
    "tags",
];

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
    let mut upserts: Vec<(String, String)> = Vec::new();
    for path in store.ticket_files()? {
        let raw = std::fs::read_to_string(&path).map_err(Error::Io)?;
        let mut doc = Document::parse(&raw)?;
        // Detect drift by managed-key presence — no rendering. The overwhelming common
        // case is an already-current ticket, so skipping the two full-file `render()`s it
        // would otherwise pay is the difference at 10k+ tickets.
        if !needs_backfill(&doc) {
            unchanged += 1;
            continue;
        }
        backfill_managed_keys(&mut doc)?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        upserts.push((id.clone(), doc.render()));
        migrated.push(id);
    }
    migrated.sort();
    // One journaled multi-upsert commit so a mid-run failure cannot leave a half-migrated board.
    if !dry_run && !upserts.is_empty() {
        let plan = plan_upserts(&upserts);
        store.commit(&plan)?;
    }
    Ok(MigrateReport {
        migrated,
        unchanged,
    })
}

/// Whether `doc` is behind the current schema — i.e. [`backfill_managed_keys`] would
/// change it. A pure frontmatter-presence check ([`Document::has_key`]) over
/// [`MANAGED_KEYS`], with no rendering, so it is cheap to run across a whole board (the
/// advisory drift nudge and the migrate no-op fast path both rely on it).
#[must_use]
pub fn needs_backfill(doc: &Document) -> bool {
    MANAGED_KEYS.iter().any(|k| !doc.has_key(k))
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
