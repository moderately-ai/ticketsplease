//! The update-available check behind the update advisory.
//!
//! Like `self-update`, this embeds no HTTP/TLS stack — it shells out to `curl`,
//! following the `releases/latest` redirect and reading the final tag from the
//! effective URL (no GitHub API, so no rate limit and no auth). The result is cached
//! under the data dir and only re-probed once per `check_interval_hours`, so a normal
//! command pays nothing and the network is touched at most once a day. Every failure
//! path is silent: a missing `curl`, no network, a malformed tag — all yield "no
//! advisory", never an error or a stall beyond the short timeout.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use ticketsplease_core::config::Maintenance;

/// The redirecting "latest release" URL. A HEAD that follows redirects lands on
/// `…/releases/tag/vX.Y.Z`; we read the tag from the effective URL.
const RELEASES_LATEST: &str = "https://github.com/moderately-ai/ticketsplease/releases/latest";
/// Test/override hook: when set, use this value as the latest version and skip the
/// cache and the network entirely (also lets a user pin a check result).
const LATEST_OVERRIDE: &str = "TICKETSPLEASE_UPDATE_LATEST";
/// Hard cap on the probe so a slow network never stalls the command for long.
const PROBE_TIMEOUT_SECS: &str = "3";

/// The cached probe result.
#[derive(Debug, Serialize, Deserialize)]
struct Cached {
    /// Epoch seconds when the probe ran.
    checked_at: u64,
    /// The latest version string seen (no `v` prefix), e.g. `0.11.0`.
    latest: String,
}

/// The update advisory line, if a newer release than this binary is available. Returns
/// `None` when the check is disabled, nothing newer exists, or anything at all fails.
#[must_use]
pub fn advisory(maint: &Maintenance) -> Option<String> {
    if !maint.update_check {
        return None;
    }
    let latest = resolve_latest(maint.check_interval_hours)?;
    let current = env!("CARGO_PKG_VERSION");
    is_newer(&latest, current).then(|| {
        format!(
            "A new ticketsplease is available: {current} -> {latest}. \
             Run `tkt self-update` to upgrade.\n  \
             https://github.com/moderately-ai/ticketsplease/releases/tag/v{latest}"
        )
    })
}

/// Resolve the latest version: an override wins; else a fresh cache; else a probe
/// (whose result is cached). `None` on any failure.
fn resolve_latest(interval_hours: u64) -> Option<String> {
    if let Some(v) = std::env::var_os(LATEST_OVERRIDE).and_then(|v| v.into_string().ok()) {
        let v = v.trim().to_string();
        return (!v.is_empty()).then_some(v);
    }
    let path = cache_path();
    let now = now_secs();
    if let Some(p) = &path {
        if let Some(c) = read_cache(p) {
            if should_use_cache(c.checked_at, now, interval_hours) {
                return Some(c.latest);
            }
        }
    }
    let latest = probe_latest()?;
    if let Some(p) = &path {
        // Best-effort: a failed cache write must never break the command.
        let _ = write_cache(
            p,
            &Cached {
                checked_at: now,
                latest: latest.clone(),
            },
        );
    }
    Some(latest)
}

/// Probe the latest tag via `curl`, following the redirect and reading the effective
/// URL's trailing `vX.Y.Z`. Silent (`None`) on any failure.
fn probe_latest() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSLI", // fail on error, silent, follow redirects, HEAD only
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "--max-time",
            PROBE_TIMEOUT_SECS,
            RELEASES_LATEST,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let effective = String::from_utf8(out.stdout).ok()?;
    tag_to_version(effective.trim())
}

/// `…/releases/tag/v0.11.0` -> `0.11.0`. `None` if the tail is not a plausible version.
fn tag_to_version(effective_url: &str) -> Option<String> {
    let tail = effective_url.rsplit('/').next()?;
    let ver = tail.strip_prefix('v').unwrap_or(tail).trim();
    parse_version(ver).map(|_| ver.to_string())
}

/// Whether a cached probe is still fresh enough to reuse.
fn should_use_cache(checked_at: u64, now: u64, interval_hours: u64) -> bool {
    now.saturating_sub(checked_at) < interval_hours.saturating_mul(3600)
}

/// Parse a `MAJOR.MINOR[.PATCH]` version into a comparable tuple. A missing patch is 0;
/// a non-numeric component (e.g. a pre-release suffix) fails, so it never notifies.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = match it.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    Some((major, minor, patch))
}

/// Whether `latest` is strictly newer than `current`. Any unparseable side -> false.
fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_DATA_HOME/ticketsplease/update-check.json` (default `~/.local/share/...`),
/// alongside the canonical skill. `None` if no home can be resolved.
fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(base.join("ticketsplease").join("update-check.json"))
}

fn read_cache(path: &PathBuf) -> Option<Cached> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(path: &PathBuf, cached: &Cached) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(cached).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_compares_semver_tuples() {
        assert!(is_newer("0.11.0", "0.10.0"));
        assert!(is_newer("0.10.1", "0.10.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.10.0", "0.10.0"), "equal is not newer");
        assert!(!is_newer("0.9.0", "0.10.0"), "older is not newer");
        // A two-component version treats patch as 0.
        assert!(is_newer("0.11", "0.10.5"));
        // Garbage on either side never claims an update.
        assert!(!is_newer("garbage", "0.10.0"));
        assert!(!is_newer("0.11.0-rc1", "0.10.0"));
    }

    #[test]
    fn tag_to_version_strips_prefix_and_validates() {
        assert_eq!(
            tag_to_version("https://x/releases/tag/v0.11.0").as_deref(),
            Some("0.11.0")
        );
        assert_eq!(
            tag_to_version("https://x/releases/tag/v0.11").as_deref(),
            Some("0.11")
        );
        assert_eq!(
            tag_to_version("https://x/releases/latest"),
            None,
            "no version tail"
        );
    }

    #[test]
    fn cache_freshness_respects_the_interval() {
        // now - checked_at < interval -> fresh; a day-old check against a 24h interval
        // is on the boundary (86400 is not < 86400), so re-probe.
        assert!(
            should_use_cache(1_000, 1_000 + 3600, 24),
            "an hour old, 24h window"
        );
        assert!(
            !should_use_cache(1_000, 1_000 + 86_400, 24),
            "exactly a day, 24h window"
        );
        assert!(!should_use_cache(1_000, 1_000 + 90_000, 24), "over a day");
        // Clock skew (now < checked_at) is treated as fresh, never a re-probe storm.
        assert!(should_use_cache(9_000, 1_000, 24));
    }
}
