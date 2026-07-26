//! Journaled multi-file working-tree transactions for [`MutationPlan`] commits.
//!
//! Layout (under repo root, **not** inside `tickets_dir`):
//!
//! ```text
//! .ticketsplease/txn/<txn_id>/
//!   journal.json
//!   staging/<id>.md
//!   staging/backup/<id>.md
//! ```
//!
//! Phases: `staging` → `publishing` → `committed`. Recovery on open/commit start
//! rolls back incomplete publishes (unlink creates, restore upsert backups,
//! reverse rename_dir).

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ids;
use crate::plan::{MutationPlan, PendingMutation};
use crate::store::{self, CreateOutcome, Store};

/// Relative path under the repo root for all transaction state.
pub const TXN_ROOT: &str = ".ticketsplease/txn";

/// Result of a successful [`Store::commit`].
#[derive(Debug, Clone, Default)]
pub struct CommitReport {
    /// Per-create outcomes in plan order (Created / Unchanged).
    pub create_results: Vec<(String, CreateOutcome)>,
    /// Ticket ids that were upserted.
    pub upserted: Vec<String>,
    /// Ticket ids that were deleted.
    pub deleted: Vec<String>,
}

/// Durable journal written under `.ticketsplease/txn/<id>/journal.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    schema_version: u32,
    txn_id: String,
    phase: Phase,
    ops: Vec<JournalOp>,
    /// Ticket ids this txn successfully published with O_EXCL (rollback targets).
    created_by_txn: Vec<String>,
    /// Ticket ids whose upsert was applied (have backups to restore).
    upserted_by_txn: Vec<String>,
    /// Rename dirs successfully applied (reverse on rollback: to → from).
    renamed_by_txn: Vec<RenameRecord>,
    /// Paths deleted by this txn (best-effort; not restored — deletes are last).
    deleted_by_txn: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenameRecord {
    from: String,
    to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    Staging,
    Publishing,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum JournalOp {
    Create {
        id: String,
        stage: String,
        dest: String,
        /// When true, publish with O_EXCL; when false, Unchanged (verify only).
        excl: bool,
    },
    Upsert {
        id: String,
        stage: String,
        dest: String,
        /// Relative path under txn dir for pre-overwrite backup (empty if dest was missing).
        backup: String,
    },
    DeleteTicket {
        id: String,
        dest: String,
        /// Relative path under txn dir for pre-delete backup (empty if missing).
        #[serde(default)]
        backup: String,
    },
    DeletePath {
        path: String,
    },
    RenameDir {
        from: String,
        to: String,
    },
}

impl Store {
    /// Absolute path to `.ticketsplease/txn`.
    #[must_use]
    pub fn txn_root(&self) -> PathBuf {
        self.repo_root.join(TXN_ROOT)
    }

    /// Finish or roll back any incomplete transaction left by a crash.
    ///
    /// Safe to call on every open/commit. No-op when no txn dirs exist.
    pub fn recover_pending_txn(&self) -> Result<()> {
        let root = self.txn_root();
        if !root.is_dir() {
            return Ok(());
        }
        let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
            .map_err(Error::Io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            recover_one(self, &dir)?;
        }
        Ok(())
    }

    /// Apply a mutation plan all-or-nothing for working-tree ticket files.
    ///
    /// Supports Create, Upsert, DeleteTicket, DeletePath, RenameDir.
    /// Does **not** validate, emit events, or hold claim refs — caller already
    /// validated; this is mechanical durability.
    pub fn commit(&self, plan: &MutationPlan) -> Result<CommitReport> {
        self.recover_pending_txn()?;

        if plan.mutations.is_empty() {
            return Ok(CommitReport {
                create_results: plan.create_results.clone(),
                ..CommitReport::default()
            });
        }

        fs::create_dir_all(self.tickets_dir()).map_err(Error::Io)?;

        let txn_id = ids::new_id();
        let txn_dir = self.txn_root().join(&txn_id);
        fs::create_dir_all(txn_dir.join("staging").join("backup")).map_err(Error::Io)?;

        let mut journal = Journal {
            schema_version: 1,
            txn_id: txn_id.clone(),
            phase: Phase::Staging,
            ops: Vec::new(),
            created_by_txn: Vec::new(),
            upserted_by_txn: Vec::new(),
            renamed_by_txn: Vec::new(),
            deleted_by_txn: Vec::new(),
        };

        // 1. Stage all new bytes and record ops.
        for m in &plan.mutations {
            match m {
                PendingMutation::Create {
                    id,
                    contents,
                    outcome,
                } => {
                    stage_create(self, &txn_dir, &mut journal, id, contents, *outcome)?;
                }
                PendingMutation::Upsert { id, contents, path } => {
                    stage_upsert(self, &txn_dir, &mut journal, id, contents, path.as_ref())?;
                }
                PendingMutation::DeleteTicket { id } => {
                    let dest = self.path_for(id);
                    // Backup existing contents so a publishing crash can restore the file.
                    let backup = if dest.exists() {
                        let backup_rel = format!("staging/backup/delete-{id}.md");
                        let existing = fs::read_to_string(&dest).map_err(Error::Io)?;
                        write_stage(&txn_dir.join(&backup_rel), &existing)?;
                        backup_rel
                    } else {
                        String::new()
                    };
                    journal.ops.push(JournalOp::DeleteTicket {
                        id: id.clone(),
                        dest: dest.display().to_string(),
                        backup,
                    });
                }
                PendingMutation::DeletePath { path } => {
                    journal.ops.push(JournalOp::DeletePath {
                        path: path.display().to_string(),
                    });
                }
                PendingMutation::RenameDir { from, to } => {
                    journal.ops.push(JournalOp::RenameDir {
                        from: from.display().to_string(),
                        to: to.display().to_string(),
                    });
                }
            }
        }

        write_journal(&txn_dir, &journal)?;

        // 2. Publishing phase.
        journal.phase = Phase::Publishing;
        write_journal(&txn_dir, &journal)?;

        // 3. Apply ops in order.
        if let Err(e) = publish_all(self, &txn_dir, &mut journal) {
            let _ = rollback_all(self, &txn_dir, &journal);
            let _ = fs::remove_dir_all(&txn_dir);
            return Err(e);
        }

        // 4. Committed.
        journal.phase = Phase::Committed;
        let _ = write_journal(&txn_dir, &journal);
        let _ = fs::remove_dir_all(&txn_dir);

        let mut upserted = Vec::new();
        let mut deleted = Vec::new();
        for m in &plan.mutations {
            match m {
                PendingMutation::Upsert { id, .. } => upserted.push(id.clone()),
                PendingMutation::DeleteTicket { id } => deleted.push(id.clone()),
                _ => {}
            }
        }

        Ok(CommitReport {
            create_results: plan.create_results.clone(),
            upserted,
            deleted,
        })
    }
}

#[allow(clippy::too_many_arguments)] // staging context + mutation fields
fn stage_create(
    store: &Store,
    txn_dir: &Path,
    journal: &mut Journal,
    id: &str,
    contents: &str,
    outcome: CreateOutcome,
) -> Result<()> {
    let dest = store.path_for(id);
    let dest_str = dest.display().to_string();
    match outcome {
        CreateOutcome::Created => {
            let stage_rel = format!("staging/{id}.md");
            write_stage(&txn_dir.join(&stage_rel), contents)?;
            journal.ops.push(JournalOp::Create {
                id: id.to_string(),
                stage: stage_rel,
                dest: dest_str,
                excl: true,
            });
        }
        CreateOutcome::Unchanged => match fs::read_to_string(&dest) {
            Ok(existing) if existing == contents => {
                journal.ops.push(JournalOp::Create {
                    id: id.to_string(),
                    stage: String::new(),
                    dest: dest_str,
                    excl: false,
                });
            }
            Ok(_) => {
                return Err(Error::Invalid(format!(
                    "ticket `{id}` already exists with different content"
                )));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let stage_rel = format!("staging/{id}.md");
                write_stage(&txn_dir.join(&stage_rel), contents)?;
                journal.ops.push(JournalOp::Create {
                    id: id.to_string(),
                    stage: stage_rel,
                    dest: dest_str,
                    excl: true,
                });
            }
            Err(e) => return Err(Error::Io(e)),
        },
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // staging context + mutation fields
fn stage_upsert(
    store: &Store,
    txn_dir: &Path,
    journal: &mut Journal,
    id: &str,
    contents: &str,
    path: Option<&PathBuf>,
) -> Result<()> {
    let dest = path.cloned().unwrap_or_else(|| store.path_for(id));
    let dest_str = dest.display().to_string();
    let stage_rel = format!("staging/{id}.md");
    write_stage(&txn_dir.join(&stage_rel), contents)?;

    let backup = if dest.exists() {
        let backup_rel = format!("staging/backup/{id}.md");
        let existing = fs::read_to_string(&dest).map_err(Error::Io)?;
        write_stage(&txn_dir.join(&backup_rel), &existing)?;
        backup_rel
    } else {
        String::new()
    };

    journal.ops.push(JournalOp::Upsert {
        id: id.to_string(),
        stage: stage_rel,
        dest: dest_str,
        backup,
    });
    Ok(())
}

fn write_stage(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    let mut f = File::create(path).map_err(Error::Io)?;
    f.write_all(contents.as_bytes()).map_err(Error::Io)?;
    f.sync_all().map_err(Error::Io)?;
    Ok(())
}

fn write_journal(txn_dir: &Path, journal: &Journal) -> Result<()> {
    let path = txn_dir.join("journal.json");
    let tmp = txn_dir.join("journal.json.tmp");
    let body = serde_json::to_string_pretty(journal)
        .map_err(|e| Error::Internal(format!("serialize journal: {e}")))?;
    {
        let mut f = File::create(&tmp).map_err(Error::Io)?;
        f.write_all(body.as_bytes()).map_err(Error::Io)?;
        f.sync_all().map_err(Error::Io)?;
    }
    fs::rename(&tmp, &path).map_err(Error::Io)?;
    Ok(())
}

fn read_journal(txn_dir: &Path) -> Result<Journal> {
    let path = txn_dir.join("journal.json");
    let raw = fs::read_to_string(&path).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            Error::Internal(format!(
                "corrupt txn dir {}: missing journal.json",
                txn_dir.display()
            ))
        } else {
            Error::Io(e)
        }
    })?;
    // Backward-compatible: older create-only journals lack new fields.
    let mut v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::Internal(format!("corrupt journal {}: {e}", path.display())))?;
    if let Some(obj) = v.as_object_mut() {
        obj.entry("upserted_by_txn")
            .or_insert_with(|| serde_json::json!([]));
        obj.entry("renamed_by_txn")
            .or_insert_with(|| serde_json::json!([]));
        obj.entry("deleted_by_txn")
            .or_insert_with(|| serde_json::json!([]));
    }
    serde_json::from_value(v)
        .map_err(|e| Error::Internal(format!("corrupt journal {}: {e}", path.display())))
}

fn publish_all(store: &Store, txn_dir: &Path, journal: &mut Journal) -> Result<()> {
    // Clone ops to avoid borrow issues while mutating journal progress fields.
    let ops = journal.ops.clone();
    for op in &ops {
        match op {
            JournalOp::Create {
                id,
                stage,
                dest: _,
                excl,
            } => {
                let dest = store.path_for(id);
                if *excl {
                    let contents = fs::read_to_string(txn_dir.join(stage)).map_err(Error::Io)?;
                    match store::create_exclusive(&dest, &contents) {
                        Ok(()) => {
                            journal.created_by_txn.push(id.clone());
                            write_journal(txn_dir, journal)?;
                        }
                        Err(Error::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists => {
                            let existing = fs::read_to_string(&dest).map_err(Error::Io)?;
                            if existing != contents {
                                return Err(Error::Invalid(format!(
                                    "ticket `{id}` already exists with different content"
                                )));
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            JournalOp::Upsert {
                id,
                stage,
                dest,
                backup: _,
            } => {
                let dest_path = PathBuf::from(dest);
                let contents = fs::read_to_string(txn_dir.join(stage)).map_err(Error::Io)?;
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent).map_err(Error::Io)?;
                }
                store::write_atomic(&dest_path, &contents)?;
                journal.upserted_by_txn.push(id.clone());
                write_journal(txn_dir, journal)?;
            }
            JournalOp::RenameDir { from, to } => {
                let from_p = PathBuf::from(from);
                let to_p = PathBuf::from(to);
                if from_p.exists() {
                    if let Some(parent) = to_p.parent() {
                        fs::create_dir_all(parent).map_err(Error::Io)?;
                    }
                    fs::rename(&from_p, &to_p).map_err(Error::Io)?;
                    journal.renamed_by_txn.push(RenameRecord {
                        from: from.clone(),
                        to: to.clone(),
                    });
                    write_journal(txn_dir, journal)?;
                }
            }
            JournalOp::DeleteTicket {
                id: _,
                dest,
                backup: _,
            }
            | JournalOp::DeletePath { path: dest } => {
                let p = PathBuf::from(dest);
                if p.is_dir() {
                    fs::remove_dir_all(&p).map_err(Error::Io)?;
                } else if p.exists() {
                    fs::remove_file(&p).map_err(Error::Io)?;
                }
                journal.deleted_by_txn.push(dest.clone());
                write_journal(txn_dir, journal)?;
            }
        }
    }
    Ok(())
}

fn rollback_all(store: &Store, txn_dir: &Path, journal: &Journal) -> Result<()> {
    // Reverse order of effects: restore deleted ticket files, reverse renames,
    // restore upserts, unlink creates.

    // Restore deleted ticket files from pre-delete backups (ops list has backup paths).
    for op in journal.ops.iter().rev() {
        if let JournalOp::DeleteTicket { dest, backup, .. } = op {
            if backup.is_empty() {
                continue;
            }
            if !journal.deleted_by_txn.iter().any(|d| d == dest) {
                continue; // delete never applied
            }
            let backup_path = txn_dir.join(backup);
            if backup_path.exists() {
                let contents = fs::read_to_string(&backup_path).map_err(Error::Io)?;
                let dest_path = PathBuf::from(dest);
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent).map_err(Error::Io)?;
                }
                store::write_atomic(&dest_path, &contents)?;
            }
        }
    }

    for rec in journal.renamed_by_txn.iter().rev() {
        let from = PathBuf::from(&rec.from);
        let to = PathBuf::from(&rec.to);
        if to.exists() {
            let _ = fs::rename(&to, &from);
        }
    }

    // Restore upsert backups for applied upserts.
    for id in journal.upserted_by_txn.iter().rev() {
        let backup = txn_dir.join(format!("staging/backup/{id}.md"));
        let dest = store.path_for(id);
        if backup.exists() {
            let contents = fs::read_to_string(&backup).map_err(Error::Io)?;
            store::write_atomic(&dest, &contents)?;
        } else if dest.exists() {
            // Upsert created a new file (no prior backup) — remove it.
            let _ = fs::remove_file(&dest);
        }
    }

    for id in journal.created_by_txn.iter().rev() {
        let path = store.path_for(id);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn recover_one(store: &Store, txn_dir: &Path) -> Result<()> {
    let journal_path = txn_dir.join("journal.json");
    if !journal_path.exists() {
        let _ = fs::remove_dir_all(txn_dir);
        return Ok(());
    }
    let journal = read_journal(txn_dir)?;
    match journal.phase {
        Phase::Staging => {
            let _ = fs::remove_dir_all(txn_dir);
        }
        Phase::Publishing => {
            rollback_all(store, txn_dir, &journal)?;
            let _ = fs::remove_dir_all(txn_dir);
        }
        Phase::Committed => {
            let _ = fs::remove_dir_all(txn_dir);
        }
    }
    Ok(())
}

/// Build a MutationPlan of upserts from full post-image ticket renders.
pub fn plan_upserts(tickets: &[(String, String)]) -> MutationPlan {
    let mut plan = MutationPlan::default();
    for (id, contents) in tickets {
        plan.mutations.push(PendingMutation::Upsert {
            id: id.clone(),
            contents: contents.clone(),
            path: None,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{plan_creates, CreateSpec, TicketRenderer};
    use crate::store::init_repo;
    use crate::ticket::Priority;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_repo() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(
            root,
            "tickets",
            &store::default_config_template("tickets"),
            false,
        )
        .unwrap();
        let store = Store::open(root).unwrap();
        (dir, store)
    }

    fn ticket_body(id: &str, title: &str, body: &str) -> String {
        crate::ticket::Ticket::new(
            id,
            title,
            "todo",
            Priority::P2,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            body,
        )
        .unwrap()
        .render()
    }

    fn spec(title: &str, id: Option<&str>) -> CreateSpec {
        CreateSpec {
            id: id.map(str::to_string),
            title: title.into(),
            status: "todo".into(),
            priority: Priority::P2,
            depends_on: vec![],
            related: vec![],
            scopes: vec![],
            shared_scopes: vec![],
            paths: vec![],
            tags: vec![],
            body: format!("body for {}\n", title),
            template: None,
        }
    }

    #[test]
    fn commit_creates_multiple_tickets() {
        let (_tmp, store) = temp_repo();
        let snap = store.snapshot_for_plan().unwrap();
        let specs = vec![spec("Alpha", Some("alpha")), spec("Beta", Some("beta"))];
        let mut r = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut r).unwrap();
        let report = store.commit(&plan).unwrap();
        assert_eq!(report.create_results.len(), 2);
        assert!(store.path_for("alpha").exists());
        assert!(store.path_for("beta").exists());
        assert!(!store.txn_root().exists() || fs::read_dir(store.txn_root()).unwrap().count() == 0);
    }

    #[test]
    fn commit_unchanged_does_not_delete_existing() {
        let (_tmp, store) = temp_repo();
        let snap = store.snapshot_for_plan().unwrap();
        let specs = vec![spec("Same", Some("same"))];
        let mut r = TicketRenderer;
        let plan = plan_creates(&snap, &specs, &mut r).unwrap();
        store.commit(&plan).unwrap();
        let before = fs::read_to_string(store.path_for("same")).unwrap();

        let snap2 = store.snapshot_for_plan().unwrap();
        let plan2 = plan_creates(&snap2, &specs, &mut r).unwrap();
        assert_eq!(plan2.create_results[0].1, CreateOutcome::Unchanged);
        store.commit(&plan2).unwrap();
        let after = fs::read_to_string(store.path_for("same")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn commit_conflict_mid_batch_rolls_back() {
        let (_tmp, store) = temp_repo();
        store
            .create_exact("mid", &ticket_body("mid", "Existing", "old\n"))
            .unwrap();

        let mut plan = MutationPlan::default();
        plan.mutations.push(PendingMutation::Create {
            id: "alpha".into(),
            contents: ticket_body("alpha", "Alpha", "a\n"),
            outcome: CreateOutcome::Created,
        });
        plan.mutations.push(PendingMutation::Create {
            id: "mid".into(),
            contents: ticket_body("mid", "Different", "new\n"),
            outcome: CreateOutcome::Created,
        });
        plan.create_results = vec![
            ("alpha".into(), CreateOutcome::Created),
            ("mid".into(), CreateOutcome::Created),
        ];

        let err = store.commit(&plan).unwrap_err();
        assert!(
            err.message().contains("different content") || matches!(err, Error::Invalid(_)),
            "{err}"
        );
        assert!(
            !store.path_for("alpha").exists(),
            "partial create must roll back"
        );
        let mid = fs::read_to_string(store.path_for("mid")).unwrap();
        assert!(mid.contains("old"), "{mid}");
    }

    #[test]
    fn commit_upserts_multiple_tickets() {
        let (_tmp, store) = temp_repo();
        store
            .create_exact("a", &ticket_body("a", "A", "old-a\n"))
            .unwrap();
        store
            .create_exact("b", &ticket_body("b", "B", "old-b\n"))
            .unwrap();

        let plan = plan_upserts(&[
            ("a".into(), ticket_body("a", "A", "new-a\n")),
            ("b".into(), ticket_body("b", "B", "new-b\n")),
        ]);
        let report = store.commit(&plan).unwrap();
        assert_eq!(report.upserted, vec!["a", "b"]);
        assert!(fs::read_to_string(store.path_for("a"))
            .unwrap()
            .contains("new-a"));
        assert!(fs::read_to_string(store.path_for("b"))
            .unwrap()
            .contains("new-b"));
    }

    #[test]
    fn upsert_failure_restores_prior_content() {
        let (_tmp, store) = temp_repo();
        store
            .create_exact("a", &ticket_body("a", "A", "old-a\n"))
            .unwrap();
        store
            .create_exact("b", &ticket_body("b", "B", "old-b\n"))
            .unwrap();

        // Manually stage a publishing journal mid-upsert to test recovery.
        let txn_id = "test-upsert-recover";
        let txn_dir = store.txn_root().join(txn_id);
        fs::create_dir_all(txn_dir.join("staging/backup")).unwrap();
        let new_a = ticket_body("a", "A", "new-a\n");
        write_stage(&txn_dir.join("staging/a.md"), &new_a).unwrap();
        write_stage(
            &txn_dir.join("staging/backup/a.md"),
            &ticket_body("a", "A", "old-a\n"),
        )
        .unwrap();
        // Apply the upsert to disk (simulating mid-publish).
        store::write_atomic(&store.path_for("a"), &new_a).unwrap();
        assert!(fs::read_to_string(store.path_for("a"))
            .unwrap()
            .contains("new-a"));

        let journal = Journal {
            schema_version: 1,
            txn_id: txn_id.into(),
            phase: Phase::Publishing,
            ops: vec![JournalOp::Upsert {
                id: "a".into(),
                stage: "staging/a.md".into(),
                dest: store.path_for("a").display().to_string(),
                backup: "staging/backup/a.md".into(),
            }],
            created_by_txn: vec![],
            upserted_by_txn: vec!["a".into()],
            renamed_by_txn: vec![],
            deleted_by_txn: vec![],
        };
        write_journal(&txn_dir, &journal).unwrap();

        store.recover_pending_txn().unwrap();
        let restored = fs::read_to_string(store.path_for("a")).unwrap();
        assert!(
            restored.contains("old-a"),
            "must restore backup: {restored}"
        );
        assert!(fs::read_to_string(store.path_for("b"))
            .unwrap()
            .contains("old-b"));
        assert!(!txn_dir.exists());
    }

    #[test]
    fn recover_publishing_rolls_back_creates() {
        let (_tmp, store) = temp_repo();
        let txn_id = "test-txn-recover";
        let txn_dir = store.txn_root().join(txn_id);
        fs::create_dir_all(txn_dir.join("staging")).unwrap();

        let contents = ticket_body("alpha", "Alpha", "a\n");
        store.create_exact("alpha", &contents).unwrap();
        assert!(store.path_for("alpha").exists());

        let journal = Journal {
            schema_version: 1,
            txn_id: txn_id.into(),
            phase: Phase::Publishing,
            ops: vec![JournalOp::Create {
                id: "alpha".into(),
                stage: "staging/alpha.md".into(),
                dest: store.path_for("alpha").display().to_string(),
                excl: true,
            }],
            created_by_txn: vec!["alpha".into()],
            upserted_by_txn: vec![],
            renamed_by_txn: vec![],
            deleted_by_txn: vec![],
        };
        write_journal(&txn_dir, &journal).unwrap();

        store.recover_pending_txn().unwrap();
        assert!(
            !store.path_for("alpha").exists(),
            "recovery must unlink created_by_txn"
        );
        assert!(!txn_dir.exists(), "txn dir cleaned after recovery");
    }

    #[test]
    fn recover_staging_discards_without_touching_board() {
        let (_tmp, store) = temp_repo();
        store
            .create_exact("keep", &ticket_body("keep", "Keep", "k\n"))
            .unwrap();

        let txn_dir = store.txn_root().join("staging-only");
        fs::create_dir_all(txn_dir.join("staging")).unwrap();
        let journal = Journal {
            schema_version: 1,
            txn_id: "staging-only".into(),
            phase: Phase::Staging,
            ops: vec![],
            created_by_txn: vec![],
            upserted_by_txn: vec![],
            renamed_by_txn: vec![],
            deleted_by_txn: vec![],
        };
        write_journal(&txn_dir, &journal).unwrap();
        store.recover_pending_txn().unwrap();
        assert!(store.path_for("keep").exists());
        assert!(!txn_dir.exists());
    }

    #[test]
    fn commit_delete_ticket_and_path() {
        let (_tmp, store) = temp_repo();
        store
            .create_exact("gone", &ticket_body("gone", "Gone", "x\n"))
            .unwrap();
        let comments = store.comments_dir("gone");
        fs::create_dir_all(&comments).unwrap();
        fs::write(comments.join("c.md"), "hi\n").unwrap();

        let mut plan = MutationPlan::default();
        plan.mutations
            .push(PendingMutation::DeleteTicket { id: "gone".into() });
        plan.mutations.push(PendingMutation::DeletePath {
            path: comments.clone(),
        });
        let report = store.commit(&plan).unwrap();
        assert_eq!(report.deleted, vec!["gone"]);
        assert!(!store.path_for("gone").exists());
        assert!(!comments.exists());
    }

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    mod tempfile {
        use super::*;
        use std::path::PathBuf;

        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
        pub fn tempdir() -> std::io::Result<TempDir> {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("tkt-txn-test-{}-{}", std::process::id(), n));
            fs::create_dir_all(&path)?;
            Ok(TempDir { path })
        }
    }
}
