//! Maintenance advisories: strictly-gated, stderr-only hints emitted *after* a
//! command completes (an update is available, the repo has drifted, the board has
//! lint findings). They exist to break the "silent staleness" failure mode without
//! compromising the tool's agent-first contract.
//!
//! By construction they are invisible to non-interactive use — see [`is_context`].
//! Nothing here ever writes to stdout (the parseable data channel) or blocks; the
//! notices go to stderr, after the command's own output, only in an interactive
//! human session.

use std::io::IsTerminal;
use std::path::Path;

use ticketsplease_core::config::Maintenance;
use ticketsplease_core::{migrate, Store};

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
pub fn run(repo: &Path, fmt: Format) {
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
    emit(&collect(repo, store.as_ref(), &maint));
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
fn collect(repo: &Path, store: Option<&Store>, maint: &Maintenance) -> Vec<String> {
    let mut lines = Vec::new();
    if std::env::var_os(SMOKE).is_some() {
        lines.push("advisory-smoke: the advisory pipe is wired".to_string());
    }
    if let Some(line) = update_check::advisory(maint) {
        lines.push(line);
    }
    // Repo-scoped sources reuse the already-open store; skipped entirely outside a repo.
    if let Some(store) = store {
        if let Some(line) = drift_advisory(repo, store) {
            lines.push(line);
        }
    }
    lines
}

/// Detect repo drift — cheaply and offline — and nudge to `migrate`: tickets whose
/// managed frontmatter is behind the current schema (a dry-run migrate's would-change
/// count) and/or a stale project skill link (a real copy or wrong link, not a symlink to
/// the canonical copy). Silent when the board is current and the link is healthy.
fn drift_advisory(repo: &Path, store: &Store) -> Option<String> {
    let behind = migrate::migrate(store, true)
        .map(|r| r.migrated.len())
        .unwrap_or(0);
    let link_path = skill::project_path(repo, ".claude/skills");
    let link_stale =
        std::fs::symlink_metadata(&link_path).is_ok() && !skill::link_ok(repo, ".claude/skills");
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
