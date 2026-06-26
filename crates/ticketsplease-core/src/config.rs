//! Repository configuration: `ticketsplease.toml` at the repository root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Config file name, located at the repository root.
pub const CONFIG_FILE: &str = "ticketsplease.toml";

/// Default tickets directory, relative to the repository root.
pub const DEFAULT_TICKETS_DIR: &str = "tickets";

/// Parsed `ticketsplease.toml`.
///
/// Scope maps use `BTreeMap` so iteration order is deterministic (R13).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Frontmatter/config schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Directory holding ticket markdown files (relative to the repo root).
    #[serde(default = "default_tickets_dir")]
    pub tickets_dir: String,
    /// Optional pin: the ticketsplease binary version this repo expects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_version: Option<String>,
    /// Default base ref for `guard` diffs.
    #[serde(default = "default_base")]
    pub default_base: String,
    /// Language-backend configuration.
    #[serde(default)]
    pub language: Language,
    /// Abstract scope name -> path globs.
    #[serde(default)]
    pub scopes: BTreeMap<String, Vec<String>>,
    /// Scope name -> owning crate (lets the Rust backend expand reverse-deps).
    #[serde(default)]
    pub scope_crates: BTreeMap<String, String>,
    /// Scope name -> external/forked dependency descriptor. Lets the guard flag a
    /// branch that bumps a pinned `git = … rev = …` dependency (or edits an
    /// in-tree fork path) against tickets that declare the same external scope.
    #[serde(default)]
    pub external_scopes: BTreeMap<String, ExternalScope>,
}

/// An external/forked dependency that lives outside this repo (pinned via
/// `git = "…" rev = "…"`), expressed as a named scope tickets can declare.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalScope {
    /// Upstream repo identifier, matched as a substring against changed manifest
    /// (`Cargo.toml`/`Cargo.lock`) lines — e.g. `"tomsanbear/sqlparser"`.
    pub repo: String,
    /// Optional in-tree globs for a vendored / path-dependency fork. Empty means
    /// the scope fires only on a manifest pin change keyed by `repo`.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Language-backend selection for the conflict guard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Language {
    /// Which diff -> scope backend to use.
    #[serde(default)]
    pub backend: Backend,
}

/// Supported language backends for diff -> scope mapping (R10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Path-glob mapping only (language-agnostic).
    #[default]
    None,
    /// Rust/cargo crate-graph expansion (requires `cargo` at runtime).
    Rust,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            tickets_dir: default_tickets_dir(),
            required_version: None,
            default_base: default_base(),
            language: Language::default(),
            scopes: BTreeMap::new(),
            scope_crates: BTreeMap::new(),
            external_scopes: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Load and parse `<repo_root>/ticketsplease.toml`.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", path.display())))?;
        toml::from_str(&text).map_err(|e| Error::Invalid(format!("invalid {CONFIG_FILE}: {e}")))
    }

    /// Absolute path to the tickets directory.
    #[must_use]
    pub fn tickets_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(&self.tickets_dir)
    }
}

fn default_schema_version() -> u32 {
    1
}

fn default_tickets_dir() -> String {
    DEFAULT_TICKETS_DIR.to_string()
}

fn default_base() -> String {
    "main".to_string()
}
