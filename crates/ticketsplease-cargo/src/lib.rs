//! Rust/cargo crate-graph backend for the ticketsplease conflict guard.
//!
//! Maps changed files to affected scopes by walking the cargo crate graph
//! (reverse-dependents): a change to a leaf crate flags every crate that
//! transitively depends on it, then those crates map back to scopes via the
//! repo's `[scope_crates]` config. Requires `cargo` on `PATH` at runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use guppy::graph::DependencyDirection;
use guppy::{MetadataCommand, PackageId};

use ticketsplease_core::guard::{merge_cause, AffectedSetMapper, ScopeCause};
use ticketsplease_core::{Error, Result};

/// Maps changed files to scopes via the cargo crate graph and reverse-dependents.
pub struct CargoMapper {
    repo: PathBuf,
    /// Inverted `[scope_crates]`: crate name -> the scopes it backs.
    crate_to_scopes: BTreeMap<String, Vec<String>>,
    /// Scopes that also have path globs. For these the [`PathGlobMapper`] is the
    /// authority on a *direct* (file) touch, so the crate graph only ever marks
    /// them transitive — otherwise a change to one sub-crate scope would falsely
    /// mark every sibling scope sharing the crate as directly touched.
    glob_scopes: BTreeSet<String>,
    /// When true, gate on the crates that own changed files only — skip the
    /// reverse-dependency expansion (and the scopes it would add transitively).
    direct_only: bool,
}

impl CargoMapper {
    /// Build from the repo root, the config's `scope -> crate` map, and the set of
    /// scopes that have path globs. When `direct_only` is set, the reverse-dependency
    /// walk is skipped.
    #[must_use]
    pub fn new(
        repo: &Path,
        scope_crates: &BTreeMap<String, String>,
        glob_scopes: &BTreeSet<String>,
        direct_only: bool,
    ) -> Self {
        let mut crate_to_scopes: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (scope, krate) in scope_crates {
            crate_to_scopes
                .entry(krate.clone())
                .or_default()
                .push(scope.clone());
        }
        Self {
            repo: repo.to_path_buf(),
            crate_to_scopes,
            glob_scopes: glob_scopes.clone(),
            direct_only,
        }
    }

    /// The cause for a scope reached from the crate graph. A scope is `Direct` only
    /// when its crate owns a changed file AND the scope has no globs (so the crate
    /// mapping is its only signal); a glob-defined scope is left to the
    /// `PathGlobMapper` and so is at most `Transitive` here.
    fn crate_scope_cause(&self, scope: &str, is_seed: bool) -> ScopeCause {
        if is_seed && !self.glob_scopes.contains(scope) {
            ScopeCause::Direct
        } else {
            ScopeCause::Transitive
        }
    }
}

impl AffectedSetMapper for CargoMapper {
    fn map(&self, changed_files: &[String]) -> Result<BTreeMap<String, ScopeCause>> {
        // No crate→scope mapping configured ⇒ nothing this backend can add.
        if self.crate_to_scopes.is_empty() {
            return Ok(BTreeMap::new());
        }

        let graph = MetadataCommand::new()
            .current_dir(&self.repo)
            .build_graph()
            .map_err(|e| Error::Invalid(format!("cargo metadata failed: {e}")))?;

        let workspace = graph.workspace();
        let root = workspace.root();

        // (relative crate dir, package id, package name) per workspace member.
        let mut members: Vec<(String, &PackageId, String)> = Vec::new();
        for pkg in workspace.iter() {
            let dir = pkg.manifest_path().parent().unwrap_or(root);
            let rel = dir
                .strip_prefix(root)
                .map(|p| p.as_str().to_string())
                .unwrap_or_default();
            members.push((rel, pkg.id(), pkg.name().to_string()));
        }

        // Seed crates: the workspace member owning each changed file (longest match).
        // Track names too, so reverse-dep results can be classified direct vs transitive.
        let mut seeds: BTreeSet<&PackageId> = BTreeSet::new();
        let mut seed_names: BTreeSet<&str> = BTreeSet::new();
        for file in changed_files {
            let mut best: Option<(usize, &PackageId, &str)> = None;
            for (rel, id, name) in &members {
                if file_under(file, rel) {
                    let len = rel.len();
                    let better = match best {
                        Some((blen, _, _)) => len > blen,
                        None => true,
                    };
                    if better {
                        best = Some((len, id, name.as_str()));
                    }
                }
            }
            if let Some((_, id, name)) = best {
                seeds.insert(id);
                seed_names.insert(name);
            }
        }
        if seeds.is_empty() {
            return Ok(BTreeMap::new());
        }

        let mut scopes: BTreeMap<String, ScopeCause> = BTreeMap::new();

        // `--direct-only`: the changed crates themselves are a direct file overlap;
        // skip the reverse-dependency walk. Only crate-only scopes are emitted —
        // glob-defined scopes are the PathGlobMapper's authority, so an unmatched
        // sibling sharing the crate must not appear.
        if self.direct_only {
            for name in &seed_names {
                if let Some(s) = self.crate_to_scopes.get(*name) {
                    for scope in s {
                        if !self.glob_scopes.contains(scope) {
                            merge_cause(&mut scopes, scope.clone(), ScopeCause::Direct);
                        }
                    }
                }
            }
            return Ok(scopes);
        }

        // Reverse-reachable set: the seeds plus every crate that depends on them.
        let query = graph
            .query_reverse(seeds.iter().copied())
            .map_err(|e| Error::Invalid(format!("guppy reverse query failed: {e}")))?;
        let resolved = query.resolve();

        for pkg in resolved.packages(DependencyDirection::Reverse) {
            if let Some(s) = self.crate_to_scopes.get(pkg.name()) {
                let is_seed = seed_names.contains(pkg.name());
                for scope in s {
                    let cause = self.crate_scope_cause(scope, is_seed);
                    merge_cause(&mut scopes, scope.clone(), cause);
                }
            }
        }
        Ok(scopes)
    }
}

/// A workspace member, for seeding scope config on `init`.
pub struct WorkspaceMember {
    /// Crate name.
    pub name: String,
    /// Crate directory relative to the workspace root (empty for a root crate).
    pub rel_dir: String,
}

/// List the workspace members (name + relative dir), sorted by name. Runs
/// `cargo metadata`, so it requires `cargo` on `PATH`.
pub fn workspace_members(repo: &Path) -> Result<Vec<WorkspaceMember>> {
    let graph = MetadataCommand::new()
        .current_dir(repo)
        .build_graph()
        .map_err(|e| Error::Invalid(format!("cargo metadata failed: {e}")))?;
    let workspace = graph.workspace();
    let root = workspace.root();
    let mut out = Vec::new();
    for pkg in workspace.iter() {
        let dir = pkg.manifest_path().parent().unwrap_or(root);
        let rel = dir
            .strip_prefix(root)
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        out.push(WorkspaceMember {
            name: pkg.name().to_string(),
            rel_dir: rel,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn file_under(file: &str, crate_rel_dir: &str) -> bool {
    if crate_rel_dir.is_empty() {
        // A crate rooted at the workspace root owns anything not under a sub-crate.
        return true;
    }
    file == crate_rel_dir || file.starts_with(&format!("{crate_rel_dir}/"))
}
