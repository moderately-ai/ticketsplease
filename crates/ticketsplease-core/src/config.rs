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
    /// Scope name -> scheduling policy. Tunes how costly an *exclusive* overlap on
    /// that scope is for `tracks`/`next` (`--max-overlap`): `weight = 0` makes a scope
    /// free to co-edit (an always-shareable hub), a higher weight makes overlaps there
    /// count for more. Default weight is 1; a shared-by-both claim is always free.
    #[serde(default)]
    pub scope_policy: BTreeMap<String, ScopePolicy>,
}

/// Per-scope scheduling policy (see [`Config::scope_policy`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopePolicy {
    /// Conflict-cost weight for an exclusive overlap on this scope. `0` = free to
    /// share; higher = riskier. Default 1.
    #[serde(default = "default_weight")]
    pub weight: i64,
}

impl Config {
    /// The scope -> conflict-cost-weight map (scopes without a policy default to 1).
    #[must_use]
    pub fn scope_weights(&self) -> BTreeMap<String, i64> {
        self.scope_policy
            .iter()
            .map(|(k, v)| (k.clone(), v.weight))
            .collect()
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Language {
    /// Which diff -> scope backend to use.
    #[serde(default)]
    pub backend: Backend,
    /// When false, the guard skips the cargo reverse-dependency expansion by
    /// default (as if every `guard` ran with `--direct-only`) — useful in a
    /// workspace with a foundational crate that most others depend on, where the
    /// transitive impact is noise more often than signal. Default true.
    #[serde(default = "default_true")]
    pub reverse_dep_expansion: bool,
}

impl Default for Language {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            reverse_dep_expansion: true,
        }
    }
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
            scope_policy: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Load and parse `<repo_root>/ticketsplease.toml`.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Invalid(format!(
                    "not initialized: no {CONFIG_FILE} in {} (run `tkt init`)",
                    repo_root.display()
                ))
            } else {
                Error::Invalid(format!("cannot read {}: {e}", path.display()))
            }
        })?;
        Self::parse(&text)
    }

    /// Parse config from raw TOML text (e.g. the blob committed on a git ref).
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| Error::Invalid(format!("invalid {CONFIG_FILE}: {e}")))
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

fn default_true() -> bool {
    true
}

fn default_weight() -> i64 {
    1
}

fn default_tickets_dir() -> String {
    DEFAULT_TICKETS_DIR.to_string()
}

fn default_base() -> String {
    "main".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_policy_weights_parse() {
        let c: Config =
            toml::from_str("[scope_policy]\ncore = { weight = 0 }\n\"q/p\" = { weight = 3 }\n")
                .unwrap();
        let w = c.scope_weights();
        assert_eq!(w.get("core"), Some(&0));
        assert_eq!(w.get("q/p"), Some(&3));
        // An entry that omits `weight` defaults to 1.
        let c2: Config = toml::from_str("[scope_policy]\ncore = {}\n").unwrap();
        assert_eq!(c2.scope_weights().get("core"), Some(&1));
    }

    #[test]
    fn reverse_dep_expansion_defaults_on() {
        // Guards the manual `Default` impl: a derived one would give `false`.
        assert!(Config::default().language.reverse_dep_expansion);
        let omitted: Config = toml::from_str("[language]\nbackend = \"rust\"\n").unwrap();
        assert!(omitted.language.reverse_dep_expansion, "omitted -> true");
        let off: Config = toml::from_str("[language]\nreverse_dep_expansion = false\n").unwrap();
        assert!(!off.language.reverse_dep_expansion);
    }
}
