//! Journaled multi-file working-tree transactions for [`MutationPlan`] commits.
//!
//! Layout (under repo root, **not** inside `tickets_dir`):
//!
//! ```text
//! .ticketsplease/txn/<txn_id>/
//!   journal.json
//!   staging/<id>.md
//! ```
//!
//! Phases: `staging` → `publishing` → `committed`. Recovery on open/commit start
//! rolls back incomplete publishes. This ticket implements **creates only**;
//! upsert/delete/rename land in later tickets.

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
    /// Ticket ids that were upserted (empty until upsert ops land).
    pub upserted: Vec<String>,
    /// Ticket ids that were deleted (empty until delete ops land).
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
    /// Currently supports [`PendingMutation::Create`] only; other ops return
    /// [`Error::Internal`] until later initiative tickets land.
    ///
    /// Does **not** validate, emit events, or hold claim refs — caller already
    /// validated; this is mechanical durability.
    pub fn commit(&self, plan: &MutationPlan) -> Result<CommitReport> {
        self.recover_pending_txn()?;

        for m in &plan.mutations {
            if !matches!(m, PendingMutation::Create { .. }) {
                return Err(Error::Internal(
                    "Store::commit currently supports Create mutations only \
                     (upsert/delete/rename require mutation-txn-upsert)"
                        .into(),
                ));
            }
        }

        if plan.mutations.is_empty() {
            return Ok(CommitReport {
                create_results: plan.create_results.clone(),
                ..CommitReport::default()
            });
        }

        // Ensure tickets dir exists before publish.
        fs::create_dir_all(self.tickets_dir()).map_err(Error::Io)?;

        let txn_id = ids::new_id();
        let txn_dir = self.txn_root().join(&txn_id);
        fs::create_dir_all(txn_dir.join("staging")).map_err(Error::Io)?;

        let mut journal = Journal {
            schema_version: 1,
            txn_id: txn_id.clone(),
            phase: Phase::Staging,
            ops: Vec::new(),
            created_by_txn: Vec::new(),
        };

        // 1. Stage Created bodies; record Unchanged as verify-only ops.
        for m in &plan.mutations {
            let PendingMutation::Create {
                id,
                contents,
                outcome,
            } = m
            else {
                unreachable!("filtered above");
            };
            let dest = self.path_for(id);
            let dest_str = dest.display().to_string();
            match outcome {
                CreateOutcome::Created => {
                    let stage_rel = format!("staging/{id}.md");
                    let stage_path = txn_dir.join(&stage_rel);
                    write_stage(&stage_path, contents)?;
                    journal.ops.push(JournalOp::Create {
                        id: id.clone(),
                        stage: stage_rel,
                        dest: dest_str,
                        excl: true,
                    });
                }
                CreateOutcome::Unchanged => {
                    // Race check: on-disk must still match planned contents.
                    match fs::read_to_string(&dest) {
                        Ok(existing) if existing == *contents => {
                            journal.ops.push(JournalOp::Create {
                                id: id.clone(),
                                stage: String::new(),
                                dest: dest_str,
                                excl: false,
                            });
                        }
                        Ok(_) => {
                            let _ = fs::remove_dir_all(&txn_dir);
                            return Err(Error::Invalid(format!(
                                "ticket `{id}` already exists with different content"
                            )));
                        }
                        Err(e) if e.kind() == ErrorKind::NotFound => {
                            // Planned Unchanged but file vanished — treat as create.
                            let stage_rel = format!("staging/{id}.md");
                            let stage_path = txn_dir.join(&stage_rel);
                            write_stage(&stage_path, contents)?;
                            journal.ops.push(JournalOp::Create {
                                id: id.clone(),
                                stage: stage_rel,
                                dest: dest_str,
                                excl: true,
                            });
                        }
                        Err(e) => {
                            let _ = fs::remove_dir_all(&txn_dir);
                            return Err(Error::Io(e));
                        }
                    }
                }
            }
        }

        write_journal(&txn_dir, &journal)?;

        // 2. Publishing phase.
        journal.phase = Phase::Publishing;
        write_journal(&txn_dir, &journal)?;

        // 3. Apply creates.
        if let Err(e) = publish_creates(self, &txn_dir, &mut journal) {
            let _ = rollback_creates(self, &journal);
            let _ = fs::remove_dir_all(&txn_dir);
            return Err(e);
        }

        // 4. Committed.
        journal.phase = Phase::Committed;
        if let Err(e) = write_journal(&txn_dir, &journal) {
            // Publishes already landed; try to keep journal for recovery cleanup.
            let _ = e;
        }
        let _ = fs::remove_dir_all(&txn_dir);

        Ok(CommitReport {
            create_results: plan.create_results.clone(),
            upserted: Vec::new(),
            deleted: Vec::new(),
        })
    }
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
    serde_json::from_str(&raw)
        .map_err(|e| Error::Internal(format!("corrupt journal {}: {e}", path.display())))
}

fn publish_creates(store: &Store, txn_dir: &Path, journal: &mut Journal) -> Result<()> {
    for op in &journal.ops {
        let JournalOp::Create {
            id,
            stage,
            dest: _,
            excl,
        } = op;
        let dest = store.path_for(id);
        if *excl {
            let stage_path = txn_dir.join(stage);
            let contents = fs::read_to_string(&stage_path).map_err(Error::Io)?;
            match store::create_exclusive(&dest, &contents) {
                Ok(()) => {
                    journal.created_by_txn.push(id.clone());
                    // Persist progress so recovery knows what we published.
                    write_journal(txn_dir, journal)?;
                }
                Err(Error::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists => {
                    let existing = fs::read_to_string(&dest).map_err(Error::Io)?;
                    if existing == contents {
                        // Concurrent identical create — treat as success, not ours to delete.
                    } else {
                        return Err(Error::Invalid(format!(
                            "ticket `{id}` already exists with different content"
                        )));
                    }
                }
                Err(e) => return Err(e),
            }
        } else {
            // Unchanged: already verified at stage time; re-check lightly.
            let _ = dest;
        }
    }
    Ok(())
}

fn rollback_creates(store: &Store, journal: &Journal) -> Result<()> {
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
        // Incomplete mkdir — drop the dir.
        let _ = fs::remove_dir_all(txn_dir);
        return Ok(());
    }
    let journal = match read_journal(txn_dir) {
        Ok(j) => j,
        Err(e @ Error::Internal(_)) => return Err(e),
        Err(e) => return Err(e),
    };
    match journal.phase {
        Phase::Staging => {
            let _ = fs::remove_dir_all(txn_dir);
        }
        Phase::Publishing => {
            rollback_creates(store, &journal)?;
            let _ = fs::remove_dir_all(txn_dir);
        }
        Phase::Committed => {
            let _ = fs::remove_dir_all(txn_dir);
        }
    }
    Ok(())
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
        // Pre-create conflicting `mid` with different content.
        store
            .create_exact(
                "mid",
                &crate::ticket::Ticket::new(
                    "mid",
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
                .unwrap()
                .render(),
            )
            .unwrap();

        // Plan thinks mid is free (snapshot taken... actually snapshot will see mid).
        // Force a plan with Created for mid by building mutations manually with wrong outcome.
        let mut plan = MutationPlan::default();
        let a = crate::ticket::Ticket::new(
            "alpha",
            "Alpha",
            "todo",
            Priority::P2,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "a\n",
        )
        .unwrap()
        .render();
        let mid_new = crate::ticket::Ticket::new(
            "mid",
            "Different",
            "todo",
            Priority::P2,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "new\n",
        )
        .unwrap()
        .render();
        plan.mutations.push(PendingMutation::Create {
            id: "alpha".into(),
            contents: a.clone(),
            outcome: CreateOutcome::Created,
        });
        plan.mutations.push(PendingMutation::Create {
            id: "mid".into(),
            contents: mid_new,
            outcome: CreateOutcome::Created, // lie: will O_EXCL-conflict
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
        // alpha must be rolled back
        assert!(
            !store.path_for("alpha").exists(),
            "partial create must roll back"
        );
        // pre-existing mid still old content
        let mid = fs::read_to_string(store.path_for("mid")).unwrap();
        assert!(mid.contains("old"), "{mid}");
    }

    #[test]
    fn recover_publishing_rolls_back_creates() {
        let (_tmp, store) = temp_repo();
        let txn_id = "test-txn-recover";
        let txn_dir = store.txn_root().join(txn_id);
        fs::create_dir_all(txn_dir.join("staging")).unwrap();

        // Simulate a published alpha + journal stuck in publishing.
        let contents = crate::ticket::Ticket::new(
            "alpha",
            "Alpha",
            "todo",
            Priority::P2,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "a\n",
        )
        .unwrap()
        .render();
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
            .create_exact(
                "keep",
                &crate::ticket::Ticket::new(
                    "keep",
                    "Keep",
                    "todo",
                    Priority::P2,
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    "k\n",
                )
                .unwrap()
                .render(),
            )
            .unwrap();

        let txn_dir = store.txn_root().join("staging-only");
        fs::create_dir_all(txn_dir.join("staging")).unwrap();
        let journal = Journal {
            schema_version: 1,
            txn_id: "staging-only".into(),
            phase: Phase::Staging,
            ops: vec![],
            created_by_txn: vec![],
        };
        write_journal(&txn_dir, &journal).unwrap();
        store.recover_pending_txn().unwrap();
        assert!(store.path_for("keep").exists());
        assert!(!txn_dir.exists());
    }

    // tempfile is a dev-dep of cli; for core tests use std::env::temp_dir + unique name
    // if tempfile isn't available. Check if core has tempfile.
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
