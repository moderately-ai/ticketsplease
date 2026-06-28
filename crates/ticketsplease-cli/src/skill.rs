//! The bundled Claude skill: embedded at compile time, installed once to a canonical
//! per-user location, and linked into each project.
//!
//! The skill is the exact version baked into this binary. To avoid stale per-repo
//! copies after a `self-update`, the content lives once at a canonical data-dir path
//! (`$XDG_DATA_HOME/ticketsplease/skill`), stamped with the binary version; each
//! project's `.claude/skills/ticketsplease` is a *symlink* to it, so refreshing the
//! canonical copy (which `skill sync` / the installer does) updates every linked
//! project at once. `--copy` still writes a committable real copy for those who want it.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use ticketsplease_core::{Error, Result};

/// The `skill/` directory, baked into the binary so the canonical copy is always the
/// exact version that ships with this build.
static SKILL_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skill");

/// File inside the canonical dir recording the binary version it was synced from.
const SENTINEL: &str = ".skill-version";

/// The skill version this binary carries.
#[must_use]
pub fn embedded_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Canonical skill location: `$XDG_DATA_HOME/ticketsplease/skill` (default
/// `~/.local/share/ticketsplease/skill`).
pub fn canonical_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home()?.join(".local").join("share"),
    };
    Ok(base.join("ticketsplease").join("skill"))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::Invalid("cannot resolve home directory ($HOME unset)".into()))
}

/// The version stamped in the canonical dir, if it exists.
#[must_use]
pub fn installed_version() -> Option<String> {
    let path = canonical_dir().ok()?.join(SENTINEL);
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Whether the canonical skill is present and matches this binary.
#[must_use]
pub fn is_current() -> bool {
    installed_version().as_deref() == Some(embedded_version())
}

/// Extract the embedded skill to the canonical dir (a clean overwrite) and stamp the
/// version. Idempotent; returns the canonical path.
pub fn sync() -> Result<PathBuf> {
    let dir = canonical_dir()?;
    // Wipe first so a reference file removed in a newer version doesn't linger.
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(Error::Io)?;
    }
    fs::create_dir_all(&dir).map_err(Error::Io)?;
    SKILL_DIR.extract(&dir).map_err(Error::Io)?;
    fs::write(dir.join(SENTINEL), embedded_version()).map_err(Error::Io)?;
    Ok(dir)
}

/// Ensure the canonical skill is present and current, syncing if not.
pub fn ensure_canonical() -> Result<PathBuf> {
    if is_current() {
        canonical_dir()
    } else {
        sync()
    }
}

/// The project's skill path under `base_dir` (e.g. `<repo>/.claude/skills/ticketsplease`).
#[must_use]
pub fn project_path(repo: &Path, base_dir: &str) -> PathBuf {
    repo.join(base_dir).join("ticketsplease")
}

/// Link a project to the canonical skill: `<repo>/<base_dir>/ticketsplease` becomes a
/// symlink to the canonical dir (replacing any stale real dir or wrong link). Ensures
/// the canonical copy exists first. Returns the link path.
pub fn link_into(repo: &Path, base_dir: &str) -> Result<PathBuf> {
    let canonical = ensure_canonical()?;
    let link = project_path(repo, base_dir);
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    remove_path(&link)?;
    make_symlink(&canonical, &link)?;
    Ok(link)
}

/// Write a committable real copy of the skill into the project (the `--copy` path).
pub fn copy_into(repo: &Path, base_dir: &str) -> Result<PathBuf> {
    let target = project_path(repo, base_dir);
    remove_path(&target)?;
    fs::create_dir_all(&target).map_err(Error::Io)?;
    SKILL_DIR.extract(&target).map_err(Error::Io)?;
    Ok(target)
}

/// Whether a project path is a symlink that resolves to the canonical skill dir.
#[must_use]
pub fn link_ok(repo: &Path, base_dir: &str) -> bool {
    let link = project_path(repo, base_dir);
    let is_symlink = fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink());
    if !is_symlink {
        return false;
    }
    match (
        fs::canonicalize(&link),
        canonical_dir().and_then(|d| fs::canonicalize(d).map_err(Error::Io)),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Remove a file, symlink, or directory at `path` if present (does not follow symlinks).
fn remove_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path).map_err(Error::Io),
        Ok(_) => fs::remove_file(path).map_err(Error::Io),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(Error::Io)
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, link: &Path) -> Result<()> {
    // No symlinks available: fall back to a real copy so the skill is still installed.
    fs::create_dir_all(link).map_err(Error::Io)?;
    SKILL_DIR.extract(link).map_err(Error::Io)
}
