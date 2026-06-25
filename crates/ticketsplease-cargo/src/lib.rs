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

use ticketsplease_core::guard::AffectedSetMapper;
use ticketsplease_core::{Error, Result};

/// Maps changed files to scopes via the cargo crate graph and reverse-dependents.
pub struct CargoMapper {
    repo: PathBuf,
    /// Inverted `[scope_crates]`: crate name -> the scopes it backs.
    crate_to_scopes: BTreeMap<String, Vec<String>>,
}

impl CargoMapper {
    /// Build from the repo root and the config's `scope -> crate` map.
    #[must_use]
    pub fn new(repo: &Path, scope_crates: &BTreeMap<String, String>) -> Self {
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
        }
    }
}

impl AffectedSetMapper for CargoMapper {
    fn map(&self, changed_files: &[String]) -> Result<BTreeSet<String>> {
        // No crate→scope mapping configured ⇒ nothing this backend can add.
        if self.crate_to_scopes.is_empty() {
            return Ok(BTreeSet::new());
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
        let mut seeds: BTreeSet<&PackageId> = BTreeSet::new();
        for file in changed_files {
            let mut best: Option<(usize, &PackageId)> = None;
            for (rel, id, _) in &members {
                if file_under(file, rel) {
                    let len = rel.len();
                    let better = match best {
                        Some((blen, _)) => len > blen,
                        None => true,
                    };
                    if better {
                        best = Some((len, id));
                    }
                }
            }
            if let Some((_, id)) = best {
                seeds.insert(id);
            }
        }
        if seeds.is_empty() {
            return Ok(BTreeSet::new());
        }

        // Reverse-reachable set: the seeds plus every crate that depends on them.
        let query = graph
            .query_reverse(seeds.iter().copied())
            .map_err(|e| Error::Invalid(format!("guppy reverse query failed: {e}")))?;
        let resolved = query.resolve();

        let mut scopes = BTreeSet::new();
        for pkg in resolved.packages(DependencyDirection::Reverse) {
            if let Some(s) = self.crate_to_scopes.get(pkg.name()) {
                scopes.extend(s.iter().cloned());
            }
        }
        Ok(scopes)
    }
}

fn file_under(file: &str, crate_rel_dir: &str) -> bool {
    if crate_rel_dir.is_empty() {
        // A crate rooted at the workspace root owns anything not under a sub-crate.
        return true;
    }
    file == crate_rel_dir || file.starts_with(&format!("{crate_rel_dir}/"))
}
