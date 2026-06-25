//! In-place self-update by re-running the published installer.
//!
//! Rather than embed an HTTP+TLS stack (which would dwarf the rest of the binary),
//! self-update reuses the same `install.sh` the `curl | sh` path uses — it
//! downloads, checksum-verifies, and atomically replaces the binary. This keeps
//! the tool's dependency surface minimal; it needs only `sh` and `curl`/`wget`.

use std::process::Command;

use ticketsplease_core::{Error, Result};

const INSTALL_URL: &str =
    "https://raw.githubusercontent.com/moderately-ai/ticketsplease/main/install.sh";

/// Re-run the installer. `version` pins a tag (e.g. `v0.2.0`); `None` = latest.
pub fn run(version: Option<&str>) -> Result<()> {
    let mut script = String::from("set -e; ");
    if let Some(tag) = version {
        // Basic guard: tags are simple identifiers, never shell metacharacters.
        if !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err(Error::Invalid(format!("invalid version tag `{tag}`")));
        }
        script.push_str(&format!("export TICKETSPLEASE_VERSION={tag}; "));
    }
    script.push_str(&format!("curl -fsSL {INSTALL_URL} | sh"));

    let status = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .status()
        .map_err(|e| Error::Internal(format!("failed to launch installer: {e}")))?;
    if !status.success() {
        return Err(Error::Internal("self-update failed".into()));
    }
    Ok(())
}
