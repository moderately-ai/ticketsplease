//! The bundled Claude skill, embedded at compile time and installable into a repo.

use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use ticketsplease_core::{Error, Result};

/// The `skill/` directory, baked into the binary so the installed skill is always
/// the exact version that ships with this build.
static SKILL_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skill");

/// Write the bundled skill into `<repo>/<base_dir>/ticketsplease/`. Returns the path.
pub fn install(repo: &Path, base_dir: &str) -> Result<PathBuf> {
    let target = repo.join(base_dir).join("ticketsplease");
    std::fs::create_dir_all(&target).map_err(Error::Io)?;
    SKILL_DIR.extract(&target).map_err(Error::Io)?;
    Ok(target)
}
