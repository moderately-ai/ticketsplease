//! Atomic ticket claiming.
//!
//! Two agents must never both believe they own one ticket. The atomicity comes
//! from git: `refs/ticketsplease/claim/<id>` is a short-lived **mutex**, acquired
//! with a *create-only* ref update (`git update-ref <ref> HEAD ""`, which git
//! serializes so exactly one of N racing callers wins) and held only across the
//! read-modify-write of the ticket. The durable claim itself lives in the ticket
//! frontmatter: `assignee` plus a `lease_expires_at` epoch deadline. Holding the
//! mutex across the whole RMW is what makes claiming correct — a second agent
//! waits for the mutex rather than observing a half-written claim, so it can never
//! race in and steal a claim that is still being recorded.
//!
//! The lease handles the common failure: an agent claims a ticket, then dies while
//! working. The mutex was already released after the claim, so the next claimer
//! acquires it, sees the expired lease, and takes over. The mutex lives in `.git`,
//! so this coordinates across `git worktree`s and a single checkout offline.

use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::store::Store;
use crate::Status;

/// Default claim lease (one hour): long enough for a coding-agent task, short
/// enough that a crashed agent stops blocking the ticket promptly.
pub const DEFAULT_TTL_SECS: u64 = 3600;

/// How long to wait for the claim mutex before giving up. A real claim holds it
/// for a single small-file read-modify-write (milliseconds); waiting this long and
/// still seeing it held means a crashed agent left a stale lock.
const MUTEX_WAIT_ATTEMPTS: u64 = 200;
const MUTEX_WAIT_STEP: Duration = Duration::from_millis(10);

/// Result of a successful claim.
#[derive(Debug, Clone, Serialize)]
pub struct ClaimOutcome {
    pub id: String,
    pub assignee: String,
    pub lease_expires_at: u64,
    /// Whether this claim took over a prior claim (an expired lease, or a live one
    /// via `--force`).
    pub stolen: bool,
    /// Whether this was the current holder renewing their own claim (no ownership
    /// change). Lets the caller skip emitting a duplicate claim event.
    pub renewed: bool,
}

/// Seconds since the Unix epoch. A lease is mutation state, not query output, so
/// reading the clock here does not affect output determinism.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Claim `id` for `agent` with a `ttl_secs` lease, marking it in-progress. Fails
/// with `Conflict` (exit 6) when another agent holds a live claim, or `Invalid`
/// when the ticket's status means it is not up for grabs.
pub fn claim(
    store: &Store,
    id: &str,
    agent: &str,
    ttl_secs: u64,
    force: bool,
) -> Result<ClaimOutcome> {
    let ticket = store.load(id)?; // NotFound (exit 4) if the id is unknown
    if !matches!(
        ticket.status,
        Status::Todo | Status::Ready | Status::InProgress
    ) {
        // A state conflict (the ticket is done/blocked/review), not bad input.
        return Err(Error::Conflict(format!(
            "ticket `{id}` is `{}`, not claimable (only todo/ready/in-progress can be claimed)",
            ticket.status
        )));
    }

    // Don't start work whose prerequisites aren't done — this mirrors `ready`/`next`,
    // which exclude such tickets from dispatch. Dangling deps are lint's concern, not
    // a claim blocker, so only existing, non-done dependencies gate.
    let blocking: Vec<String> = ticket
        .dependencies
        .iter()
        .filter_map(|dep| store.load(dep).ok())
        .filter(|d| d.status != Status::Done)
        .map(|d| d.id.clone())
        .collect();
    if !blocking.is_empty() {
        return Err(Error::Conflict(format!(
            "ticket `{id}` has unfinished dependencies: {} (finish them before claiming)",
            blocking.join(", ")
        )));
    }

    acquire_mutex(store, id)?;
    let result = claim_locked(store, id, agent, ttl_secs, force);
    release_mutex(&store.repo_root, id);
    result
}

/// The claim decision, made while holding the mutex so no one else can be mid-write.
fn claim_locked(
    store: &Store,
    id: &str,
    agent: &str,
    ttl_secs: u64,
    force: bool,
) -> Result<ClaimOutcome> {
    let mut ticket = store.load(id)?;
    let now = now_secs();
    let renewed = ticket.assignee.as_deref() == Some(agent);
    let stolen = match (ticket.assignee.as_deref(), ticket.lease_expires_at) {
        // Refreshing my own claim — always fine, extends the lease.
        (Some(holder), _) if holder == agent => false,
        // Someone else holds a live lease — they own it, unless we force-steal it.
        (Some(holder), Some(exp)) if exp > now => {
            if force {
                true
            } else {
                return Err(Error::Conflict(format!(
                    "ticket `{id}` is already claimed by `{holder}` (lease still live; \
                     use --force to steal)"
                )));
            }
        }
        // Someone else, but the lease has expired (or none was set) — take over.
        (Some(_), _) => true,
        // Unclaimed.
        (None, _) => false,
    };

    let lease = now.saturating_add(ttl_secs);
    ticket.set_claim(agent, lease)?;
    store.save(&ticket)?;
    Ok(ClaimOutcome {
        id: id.to_string(),
        assignee: agent.to_string(),
        lease_expires_at: lease,
        stolen,
        renewed,
    })
}

/// Release `id`'s claim: drop the lease, return it to `ready`. With `agent` set
/// and `force` false, only the current holder may release it. Releasing an
/// unclaimed ticket is a no-op success.
pub fn release(store: &Store, id: &str, agent: Option<&str>, force: bool) -> Result<bool> {
    let _ = store.load(id)?; // NotFound if the id is unknown
    acquire_mutex(store, id)?;
    let result = release_locked(store, id, agent, force);
    release_mutex(&store.repo_root, id);
    result
}

fn release_locked(store: &Store, id: &str, agent: Option<&str>, force: bool) -> Result<bool> {
    let mut ticket = store.load(id)?;
    if ticket.assignee.is_none() && ticket.status != Status::InProgress {
        return Ok(false); // nothing to release
    }
    if !force {
        match (ticket.assignee.as_deref(), agent) {
            // A named release must come from the holder.
            (Some(holder), Some(who)) if holder != who => {
                return Err(Error::Conflict(format!(
                    "ticket `{id}` is held by `{holder}`, not `{who}` (use --force to override)"
                )));
            }
            // A bare release (no --as) must not silently drop someone else's claim —
            // confirm the holder or force it.
            (Some(holder), None) => {
                return Err(Error::Conflict(format!(
                    "ticket `{id}` is held by `{holder}`; pass `--as {holder}` to confirm \
                     (or --force to override)"
                )));
            }
            _ => {}
        }
    }
    ticket.clear_claim()?;
    store.save(&ticket)?;
    Ok(true)
}

/// Acquire the claim mutex, waiting out the brief window another claim holds it.
fn acquire_mutex(store: &Store, id: &str) -> Result<()> {
    let repo = &store.repo_root;
    for _ in 0..MUTEX_WAIT_ATTEMPTS {
        match try_create_ref(repo, id)? {
            RefUpdate::Created => return Ok(()),
            RefUpdate::Exists | RefUpdate::Contended => sleep(MUTEX_WAIT_STEP),
        }
    }
    Err(Error::Conflict(format!(
        "could not acquire the claim lock for `{id}`: it stayed held for too long, \
         which usually means a crashed agent left a stale lock — clear it with \
         `git update-ref -d refs/ticketsplease/claim/{id}`"
    )))
}

fn claim_ref(id: &str) -> String {
    format!("refs/ticketsplease/claim/{id}")
}

/// How a create-only ref update resolved.
enum RefUpdate {
    /// The ref was created — we hold the mutex.
    Created,
    /// The ref already existed — another claim holds the mutex.
    Exists,
    /// Transient `.lock` contention from a concurrent git update; retry.
    Contended,
}

/// One atomic create-only ref update — the compare-and-swap at the heart of the
/// mutex. Empty `<oldvalue>` tells git the ref must not already exist.
fn try_create_ref(repo: &Path, id: &str) -> Result<RefUpdate> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["update-ref", &claim_ref(id), "HEAD", ""])
        .output()
        .map_err(|e| Error::Invalid(format!("failed to run git update-ref: {e}")))?;
    if out.status.success() {
        return Ok(RefUpdate::Created);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("already exists") {
        Ok(RefUpdate::Exists)
    } else if err.contains("cannot lock ref") || err.contains("unable to") {
        Ok(RefUpdate::Contended)
    } else {
        Err(Error::Invalid(format!(
            "git update-ref failed (is this a git repo with at least one commit?): {}",
            err.trim()
        )))
    }
}

/// Drop the mutex. Best-effort: a failure here at worst leaves a stale lock that
/// the wait-then-give-up path reports, so it must not mask the caller's result.
fn release_mutex(repo: &Path, id: &str) {
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["update-ref", "-d", &claim_ref(id)])
        .output();
}
