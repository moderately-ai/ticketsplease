//! Repository configuration: `ticketsplease.toml` at the repository root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::states::{StateDef, StateRegistry};

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
    /// Workflow: custom lifecycle states (+ categories) and optional transition rules.
    #[serde(default)]
    pub workflow: Workflow,
    /// Guard behaviour: whether a declared-area overlap with an open sibling gates.
    #[serde(default)]
    pub guard: Guard,
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

    /// Every scope name the config defines: a scope is "defined" if it has a glob
    /// mapping, an owning crate, or an external descriptor. This is the vocabulary a
    /// ticket's declared scopes are validated against (an undeclared scope is a typo).
    /// Empty means the repo is not using the scope system, so callers must not enforce.
    #[must_use]
    pub fn defined_scopes(&self) -> BTreeSet<&str> {
        self.scopes
            .keys()
            .chain(self.scope_crates.keys())
            .chain(self.external_scopes.keys())
            .map(String::as_str)
            .collect()
    }

    /// The effective workflow state registry: the built-in default states when the repo
    /// declares no `[workflow.states]`, else the configured set.
    #[must_use]
    pub fn state_registry(&self) -> StateRegistry {
        if self.workflow.states.is_empty() {
            StateRegistry::builtin()
        } else {
            StateRegistry::from_defs(&self.workflow.states)
        }
    }

    /// Whether a user-initiated `from -> to` status transition is permitted. Always `true`
    /// when `[workflow] enforce_transitions` is off (the default, any-to-any) or the change
    /// is a no-op; otherwise `true` only for an explicit `[workflow.transitions]` edge or a
    /// `"*"` wildcard source. Case-insensitive. Engine-driven transitions (claim/release)
    /// do not route through here and are never gated.
    #[must_use]
    pub fn can_transition(&self, from: &str, to: &str) -> bool {
        if !self.workflow.enforce_transitions || from.eq_ignore_ascii_case(to) {
            return true;
        }
        let permits = |src: &str| {
            self.workflow
                .transitions
                .get(src)
                .is_some_and(|targets| targets.iter().any(|t| t.eq_ignore_ascii_case(to)))
        };
        permits(&from.trim().to_ascii_lowercase()) || permits("*")
    }

    /// The legal target states from `from` under `[workflow.transitions]` (explicit edges
    /// plus any `"*"` wildcard), for error messages. Sorted and deduped.
    #[must_use]
    pub fn legal_transitions(&self, from: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .workflow
            .transitions
            .get(&from.trim().to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        if let Some(wild) = self.workflow.transitions.get("*") {
            out.extend(wild.iter().cloned());
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Guard configuration (`[guard]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Guard {
    /// Whether a declared-area overlap with an open sibling ticket gates the guard
    /// (exit 6). Default `false`: an overlap is a non-failing `WARN`, since under a
    /// parallel-dispatch workflow it is the expected state, not a proven merge conflict.
    /// Under-declaration (a scope escape) always gates regardless of this. Set `true`
    /// (or pass `--strict` per-invocation) to restore hard-fail-on-overlap.
    #[serde(default)]
    pub gate_collisions: bool,
}

/// Workflow configuration (`[workflow]`): custom lifecycle states and optional
/// transition enforcement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workflow {
    /// Custom state definitions (`[workflow.states.<name>]`). Empty → the built-in states.
    #[serde(default)]
    pub states: BTreeMap<String, StateDef>,
    /// Whether to enforce the `transitions` adjacency graph. Default off (any-to-any).
    /// Consumed by the transition-enforcement feature.
    #[serde(default)]
    pub enforce_transitions: bool,
    /// Allowed `state -> [states]` transitions. Only consulted when `enforce_transitions`.
    #[serde(default)]
    pub transitions: BTreeMap<String, Vec<String>>,
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
            workflow: Workflow::default(),
            guard: Guard::default(),
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

    #[test]
    fn transitions_enforced_only_when_enabled() {
        // Enforcement off (the default): any transition is allowed.
        let off = Config::default();
        assert!(off.can_transition("todo", "done"));

        let on: Config = toml::from_str(
            "[workflow]\nenforce_transitions = true\n\
             [workflow.transitions]\ntodo = [\"in-progress\"]\n\"*\" = [\"closed\"]\n",
        )
        .unwrap();
        assert!(on.can_transition("todo", "in-progress")); // explicit edge
        assert!(on.can_transition("review", "closed")); // "*" wildcard
        assert!(on.can_transition("todo", "todo")); // no-op is always allowed
        assert!(on.can_transition("TODO", "In-Progress")); // case-insensitive
        assert!(!on.can_transition("todo", "review")); // not permitted
        assert_eq!(on.legal_transitions("todo"), vec!["closed", "in-progress"]);
    }
}
