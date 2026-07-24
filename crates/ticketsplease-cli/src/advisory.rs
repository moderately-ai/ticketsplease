//! Maintenance advisories: strictly-gated, stderr-only hints emitted *after* a
//! command completes (an update is available, the repo has drifted, the board has
//! lint findings). They exist to break the "silent staleness" failure mode without
//! compromising the tool's agent-first contract.
//!
//! By construction they are invisible to non-interactive use — see [`is_context`].
//! Nothing here ever writes to stdout (the parseable data channel) or blocks; the
//! notices go to stderr, after the command's own output, only in an interactive
//! human session.

use std::hash::{Hash, Hasher};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use ticketsplease_core::config::Maintenance;
use ticketsplease_core::{lint, migrate, Store};

use crate::format::Format;
use crate::{skill, update_check};

/// Suppress every advisory (honours the common "no update notifier" convention).
const OPT_OUT: &str = "TICKETSPLEASE_NO_ADVISORIES";
/// Testing/demo override: force the TTY gates true (never overrides format / CI /
/// opt-out). Lets the advisory pipe be exercised where there is no real terminal.
const FORCE: &str = "TICKETSPLEASE_ADVISORY_FORCE";
/// Self-test source: when set (and in context), emit one recognisable smoke line so
/// the end-to-end pipe can be verified. Real sources (update-check, drift, lint) are
/// added by later tickets and plug into [`collect`].
const SMOKE: &str = "TICKETSPLEASE_ADVISORY_SMOKE";

/// Run the advisory pass. Called once from `main`, after the command's output and
/// exit code are settled. A no-op unless we are in an interactive human context.
pub fn run(repo: &Path, fmt: Format, auto_doctor: bool) {
    if !is_context(fmt) {
        return;
    }
    // Open the repo once, if we are in one: repo-scoped sources (drift) reuse the store,
    // and its config carries the maintenance settings. Outside a repo only the
    // binary-level update-check runs, with default settings.
    let store = Store::open(repo).ok();
    let maint = store
        .as_ref()
        .map(|s| s.config.maintenance.clone())
        .unwrap_or_default();
    // Auto-apply drift repair when opted in — config knob or the per-invocation flag.
    // Reachable only here, i.e. only in the interactive human context gated above; it
    // can never fire in a JSON / CI / non-TTY / parallel run.
    let apply = maint.auto_migrate || auto_doctor;
    emit(&collect(repo, store.as_ref(), &maint, apply));
}

/// Whether advisories may be shown right now: an interactive human session only.
#[must_use]
fn is_context(fmt: Format) -> bool {
    let forced = std::env::var_os(FORCE).is_some();
    gates(
        fmt,
        forced || std::io::stdout().is_terminal(),
        forced || std::io::stdin().is_terminal(),
        std::env::var_os("CI").is_some(),
        std::env::var_os(OPT_OUT).is_some(),
    )
}

/// Pure gating logic, split out so it is unit-testable without a real TTY. Advisories
/// show only in human format, on an interactive terminal (stdout **and** stdin), when
/// not under CI and not opted out.
#[must_use]
fn gates(fmt: Format, stdout_tty: bool, stdin_tty: bool, ci: bool, opted_out: bool) -> bool {
    matches!(fmt, Format::Human) && stdout_tty && stdin_tty && !ci && !opted_out
}

/// Assemble the advisory lines from each source. Sources must be cheap and silent by
/// default — they return nothing unless there is genuinely something to say.
fn collect(repo: &Path, store: Option<&Store>, maint: &Maintenance, apply: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if std::env::var_os(SMOKE).is_some() {
        lines.push("advisory-smoke: the advisory pipe is wired".to_string());
    }
    if let Some(line) = update_check::advisory(maint) {
        lines.push(line);
    }
    // Repo-scoped sources reuse the already-open store; skipped entirely outside a repo.
    if let Some(store) = store {
        if let Some(line) = drift_advisory(repo, store, apply) {
            lines.push(line);
        }
        if let Some(line) = lint_summary(repo, store, maint) {
            lines.push(line);
        }
    }
    lines
}

/// Count the board's lint findings and, if any, point to `tkt lint`. A count only —
/// never the list and never a gate. This is the signal `doctor`/`migrate` miss (a
/// `paths-without-scopes` on an open ticket, a dangling link, an unknown scope).
///
/// The count comes from the cached board scan ([`board_health`]) shared with the drift
/// nudge, so a read-only command pays at most one board walk here — and none at all when
/// the board is unchanged since the last scan.
fn lint_summary(repo: &Path, store: &Store, maint: &Maintenance) -> Option<String> {
    let n = board_health(repo, store, maint).lint;
    (n > 0).then(|| format!("board has {n} lint finding(s) — run `tkt lint`"))
}

/// Detect repo drift — cheaply and offline — and either nudge to `migrate` or, when
/// `apply` is set (opted in via config/`--auto-doctor`), repair it in place and report
/// what changed. Drift is: tickets whose managed frontmatter is behind the current
/// schema, and/or a stale project skill link (a real copy or wrong link, not a symlink to
/// the canonical copy). Silent when the board is current and the link is healthy.
///
/// `apply` performs writes, so it is correct only because this function is reached solely
/// from the interactive-human context gated in [`run`] — never in JSON / CI / non-TTY.
fn drift_advisory(repo: &Path, store: &Store, apply: bool) -> Option<String> {
    let link_path = skill::project_path(repo, ".claude/skills");
    let link_stale =
        std::fs::symlink_metadata(&link_path).is_ok() && !skill::link_ok(repo, ".claude/skills");

    if apply {
        // Auto-repair: a real migrate (backfill) plus a relink if the link is stale.
        // This path performs writes, so it never consults the cache — it does the real
        // migration (which itself bumps the board mtime and invalidates any cached count).
        let migrated = migrate::migrate(store, false)
            .map(|r| r.migrated.len())
            .unwrap_or(0);
        let relinked = link_stale && {
            skill::link_into(repo, ".claude/skills").is_ok() && {
                let _ = crate::commands::ensure_gitignored(repo, ".claude/skills/ticketsplease");
                true
            }
        };
        if migrated == 0 && !relinked {
            return None;
        }
        let mut parts = Vec::new();
        if migrated > 0 {
            parts.push(format!("migrated {migrated} ticket(s)"));
        }
        if relinked {
            parts.push("repaired the skill link".to_string());
        }
        return Some(format!("auto-migrate applied: {}", parts.join("; ")));
    }

    // Non-apply nudge: the drift count is the schema-behind count from the cached scan.
    let behind = board_health(repo, store, &store.config.maintenance).drift;
    if behind == 0 && !link_stale {
        return None;
    }
    let mut parts = Vec::new();
    if behind > 0 {
        parts.push(format!("{behind} ticket(s) need migration"));
    }
    if link_stale {
        parts.push("the skill link is stale".to_string());
    }
    Some(format!(
        "repo drifted: {} — run `tkt migrate`",
        parts.join("; ")
    ))
}

/// The board's lint-finding count and schema-drift count. Both are derived from a single
/// walk of the tickets directory ([`lint::lint_with_tickets`] yields the diagnostics and
/// the parsed tickets, off which [`migrate::needs_backfill`] counts drift), and that walk
/// is *cached* keyed on the board's mtime signature so an unchanged board is not re-scanned.
///
/// This is the fix for the advisory paying two full board parses (one for lint, one for
/// migrate) after every interactive command: it is now one parse on a real change, zero
/// on a repeat.
#[derive(Debug, Clone, Copy, Default)]
struct Health {
    /// Number of schema lint findings.
    lint: usize,
    /// Number of tickets whose frontmatter is behind the current schema.
    drift: usize,
}

fn board_health(repo: &Path, store: &Store, maint: &Maintenance) -> Health {
    let sig = board_signature(store);
    let cache = cache_path(repo);
    let now = now_secs();
    if let Some(path) = &cache {
        if let Some(c) = read_cache(path) {
            // Reuse the cached counts only when the board is byte-for-byte the same shape
            // (mtime signature unchanged) *and* the cache is within the staleness ceiling
            // — the latter bounds drift from an out-of-band edit that didn't move the dir
            // mtime (most editors rename-on-save, which does; this is the safety net).
            if c.signature == sig && fresh(c.checked_at, now, maint.check_interval_hours) {
                return Health {
                    lint: c.lint,
                    drift: c.drift,
                };
            }
        }
    }
    // Cache miss: one board walk yields both counts.
    let (diags, tickets) = lint::lint_with_tickets(store).unwrap_or_default();
    let health = Health {
        lint: diags.len(),
        drift: tickets
            .iter()
            .filter(|t| migrate::needs_backfill(t.document()))
            .count(),
    };
    if let Some(path) = &cache {
        let _ = write_cache(
            path,
            &Cached {
                signature: sig,
                checked_at: now,
                lint: health.lint,
                drift: health.drift,
            },
        );
    }
    health
}

/// A cheap fingerprint of the board's on-disk state: the `.md` file count, the
/// tickets-directory mtime, and the config-file mtime. Every `tkt` write goes through an
/// atomic temp-file + rename (or an exclusive create / a delete), each of which mutates a
/// directory entry and so bumps the tickets-dir mtime; add/remove/rename also moves the
/// count. The count is the belt to the mtime's suspenders — it catches an add/remove even
/// on a filesystem whose mtime resolution is too coarse to distinguish two edits in the
/// same tick. Counting is a single `read_dir` (no per-file `stat`), so this stays far
/// cheaper than the parse it guards.
fn board_signature(store: &Store) -> (u64, u64, u64) {
    let dir = store.tickets_dir();
    let config = store.repo_root.join("ticketsplease.toml");
    (md_file_count(&dir), mtime_nanos(&dir), mtime_nanos(&config))
}

fn md_file_count(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(std::result::Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                .count() as u64
        })
        .unwrap_or(0)
}

fn mtime_nanos(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

/// The cached board-health result for a repo.
#[derive(Debug, Serialize, Deserialize)]
struct Cached {
    /// `(md_file_count, tickets_dir_mtime, config_mtime)` when the scan ran.
    signature: (u64, u64, u64),
    /// Epoch seconds when the scan ran (staleness ceiling).
    checked_at: u64,
    /// Lint-finding count at scan time.
    lint: usize,
    /// Schema-drift count at scan time.
    drift: usize,
}

/// Whether a cached scan is still fresh enough to reuse (same logic as the update-check
/// cache: strictly within the interval, and clock skew is treated as fresh).
fn fresh(checked_at: u64, now: u64, interval_hours: u64) -> bool {
    now.saturating_sub(checked_at) < interval_hours.saturating_mul(3600)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Per-repo cache file: `$XDG_DATA_HOME/ticketsplease/advisory/<repo-hash>.json`
/// (default `~/.local/share/...`), alongside the update-check cache. Keyed by a hash of
/// the canonicalized repo path so multiple repos on one machine never collide; a hash
/// collision merely produces a signature mismatch and a harmless re-scan. `None` if no
/// home can be resolved.
fn cache_path(repo: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    let canonical = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    Some(
        base.join("ticketsplease")
            .join("advisory")
            .join(format!("{key}.json")),
    )
}

fn read_cache(path: &Path) -> Option<Cached> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &Path, cached: &Cached) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(cached).unwrap_or_default())
}

/// Emit advisory lines to stderr (never stdout). No-op when empty.
fn emit(lines: &[String]) {
    for line in lines {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_only_pass_for_interactive_human_non_ci() {
        // The one passing configuration: human, both TTYs, not CI, not opted out.
        assert!(gates(Format::Human, true, true, false, false));

        // Every single negated condition suppresses.
        assert!(!gates(Format::Json, true, true, false, false), "json");
        assert!(
            !gates(Format::Human, false, true, false, false),
            "no stdout tty"
        );
        assert!(
            !gates(Format::Human, true, false, false, false),
            "no stdin tty"
        );
        assert!(!gates(Format::Human, true, true, true, false), "CI set");
        assert!(!gates(Format::Human, true, true, false, true), "opted out");
    }
}
