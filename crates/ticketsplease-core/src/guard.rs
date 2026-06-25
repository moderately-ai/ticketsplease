//! The conflict guard (R8–R9): compute a branch's actual changed files, map them
//! to affected scopes, and reconcile against the ticket's *declared* scopes.
//!
//! Declared scopes are the seed; the computed affected set is the truth. The guard
//! fails when a branch touches scopes its ticket did not declare (under-declaration)
//! or overlaps a concurrently-open ticket (collision). The file→scope mapping is
//! pluggable via [`AffectedSetMapper`] (R10): the always-on [`PathGlobMapper`] lives
//! here; the Rust crate-graph backend lives in the `ticketsplease-cargo` crate.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::ticket::Ticket;

/// Maps a set of changed files to the abstract scopes they affect.
pub trait AffectedSetMapper {
    /// Return the scopes affected by `changed_files`.
    fn map(&self, changed_files: &[String]) -> Result<BTreeSet<String>>;
}

/// The always-on, language-agnostic mapper: match files against each scope's globs.
pub struct PathGlobMapper {
    scopes: Vec<(String, GlobSet)>,
}

impl PathGlobMapper {
    /// Build from the config's `scope -> globs` map.
    pub fn new(config: &Config) -> Result<Self> {
        let mut scopes = Vec::new();
        for (scope, globs) in &config.scopes {
            let mut builder = GlobSetBuilder::new();
            for g in globs {
                builder.add(Glob::new(g).map_err(|e| {
                    Error::Invalid(format!("invalid glob `{g}` for scope `{scope}`: {e}"))
                })?);
            }
            let set = builder
                .build()
                .map_err(|e| Error::Invalid(format!("building globset for `{scope}`: {e}")))?;
            scopes.push((scope.clone(), set));
        }
        Ok(Self { scopes })
    }
}

impl AffectedSetMapper for PathGlobMapper {
    fn map(&self, changed_files: &[String]) -> Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for (scope, set) in &self.scopes {
            if changed_files.iter().any(|f| set.is_match(f)) {
                out.insert(scope.clone());
            }
        }
        Ok(out)
    }
}

/// A branch's diff against a base: the guard's primary input.
#[derive(Debug, Clone)]
pub struct BranchDiff {
    /// Base ref diffed against.
    pub base: String,
    /// Branch (or ref) being guarded.
    pub branch: String,
    /// Changed files, sorted and deduped (repo-relative paths).
    pub changed_files: Vec<String>,
}

impl BranchDiff {
    /// Compute via a three-dot (merge-base) `git diff --name-only`. Fully offline;
    /// shells out to the system `git`.
    pub fn compute(repo: &Path, base: &str, branch: &str) -> Result<Self> {
        let range = format!("{base}...{branch}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "--name-only"])
            .arg(&range)
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Invalid(format!(
                "`git diff {range}` failed: {}",
                stderr.trim()
            )));
        }
        let mut changed_files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        changed_files.sort();
        changed_files.dedup();
        Ok(Self {
            base: base.to_string(),
            branch: branch.to_string(),
            changed_files,
        })
    }
}

/// A collision with another concurrently-open ticket.
#[derive(Debug, Clone, Serialize)]
pub struct Collision {
    /// The other ticket's id.
    pub ticket: String,
    /// Scopes shared between this branch's affected set and the other's declared set.
    pub scopes: Vec<String>,
}

/// The result of guarding a branch.
#[derive(Debug, Clone, Serialize)]
pub struct GuardReport {
    /// The ticket the branch belongs to.
    pub ticket: String,
    /// Base ref diffed against.
    pub base: String,
    /// Branch (or ref) guarded.
    pub branch: String,
    /// Changed files (sorted).
    pub changed_files: Vec<String>,
    /// Computed affected scopes (sorted).
    pub affected_scopes: Vec<String>,
    /// Scopes the ticket declared.
    pub declared_scopes: Vec<String>,
    /// Affected scopes the ticket failed to declare (under-declaration).
    pub under_declared: Vec<String>,
    /// Overlaps with other open tickets.
    pub collisions: Vec<Collision>,
    /// True if the guard found any conflict.
    pub conflict: bool,
}

/// Evaluate the guard: map changed files to scopes via `mappers`, then reconcile
/// against the target ticket's declared scopes and other open tickets.
pub fn evaluate(
    target: &Ticket,
    all: &[Ticket],
    diff: BranchDiff,
    mappers: &[&dyn AffectedSetMapper],
) -> Result<GuardReport> {
    let mut affected: BTreeSet<String> = BTreeSet::new();
    for mapper in mappers {
        affected.extend(mapper.map(&diff.changed_files)?);
    }

    let declared: BTreeSet<String> = target.scopes.iter().cloned().collect();
    let under_declared: Vec<String> = affected.difference(&declared).cloned().collect();

    let mut collisions = Vec::new();
    for other in all {
        if other.id == target.id || !other.status.is_open() {
            continue;
        }
        let other_declared: BTreeSet<String> = other.scopes.iter().cloned().collect();
        let shared: Vec<String> = affected.intersection(&other_declared).cloned().collect();
        if !shared.is_empty() {
            collisions.push(Collision {
                ticket: other.id.clone(),
                scopes: shared,
            });
        }
    }

    let conflict = !under_declared.is_empty() || !collisions.is_empty();
    Ok(GuardReport {
        ticket: target.id.clone(),
        base: diff.base,
        branch: diff.branch,
        changed_files: diff.changed_files,
        affected_scopes: affected.into_iter().collect(),
        declared_scopes: declared.into_iter().collect(),
        under_declared,
        collisions,
        conflict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, status: &str, scopes: &[&str]) -> Ticket {
        let sc: Vec<String> = scopes.iter().map(|s| (*s).to_string()).collect();
        Ticket::new(
            id,
            id,
            status.parse().unwrap(),
            "p2".parse().unwrap(),
            &[],
            &sc,
            &[],
            &[],
            "",
        )
        .unwrap()
    }

    fn config_with_scopes(pairs: &[(&str, &str)]) -> Config {
        let mut cfg = Config::default();
        for (scope, glob) in pairs {
            cfg.scopes
                .insert((*scope).to_string(), vec![(*glob).to_string()]);
        }
        cfg
    }

    fn diff(files: &[&str]) -> BranchDiff {
        BranchDiff {
            base: "main".to_string(),
            branch: "feat".to_string(),
            changed_files: files.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn path_glob_mapper_maps_files() {
        let cfg = config_with_scopes(&[("core", "core/**"), ("io", "io/**")]);
        let mapper = PathGlobMapper::new(&cfg).unwrap();
        let affected = mapper
            .map(&["core/src/lib.rs".to_string(), "docs/readme.md".to_string()])
            .unwrap();
        assert!(affected.contains("core"));
        assert!(!affected.contains("io"));
    }

    #[test]
    fn under_declaration_is_a_conflict() {
        let cfg = config_with_scopes(&[("core", "core/**"), ("io", "io/**")]);
        let mapper = PathGlobMapper::new(&cfg).unwrap();
        let target = ticket("t", "in-progress", &["core"]);
        let all = vec![target.clone()];
        let report = evaluate(&target, &all, diff(&["core/a.rs", "io/b.rs"]), &[&mapper]).unwrap();
        assert!(report.conflict);
        assert_eq!(report.under_declared, vec!["io"]);
    }

    #[test]
    fn collision_with_open_ticket() {
        let cfg = config_with_scopes(&[("core", "core/**")]);
        let mapper = PathGlobMapper::new(&cfg).unwrap();
        let target = ticket("t", "in-progress", &["core"]);
        let other = ticket("u", "in-progress", &["core"]);
        let all = vec![target.clone(), other];
        let report = evaluate(&target, &all, diff(&["core/a.rs"]), &[&mapper]).unwrap();
        assert!(report.conflict);
        assert_eq!(report.collisions.len(), 1);
        assert_eq!(report.collisions[0].ticket, "u");
    }

    #[test]
    fn clean_branch_is_ok() {
        let cfg = config_with_scopes(&[("core", "core/**")]);
        let mapper = PathGlobMapper::new(&cfg).unwrap();
        let target = ticket("t", "in-progress", &["core"]);
        let all = vec![target.clone()];
        let report = evaluate(&target, &all, diff(&["core/a.rs"]), &[&mapper]).unwrap();
        assert!(!report.conflict);
    }
}
