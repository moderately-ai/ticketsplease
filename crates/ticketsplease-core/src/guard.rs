//! The conflict guard (R8–R9): compute a branch's actual changed files, map them
//! to affected scopes, and reconcile against the ticket's *declared* scopes.
//!
//! Declared scopes are the seed; the computed affected set is the truth. The guard
//! fails when a branch touches scopes its ticket did not declare (under-declaration)
//! or overlaps a concurrently-open ticket (collision). The file→scope mapping is
//! pluggable via [`AffectedSetMapper`] (R10): the always-on [`PathGlobMapper`] lives
//! here; the Rust crate-graph backend lives in the `ticketsplease-cargo` crate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::config::{Config, ExternalScope};
use crate::error::{Error, Result};
use crate::ticket::Ticket;

/// Why a scope entered the affected set.
///
/// `Direct` means the changed files themselves fall in the scope — a path-glob
/// match, or the scope's own crate owns a changed file, or an external pin was
/// touched. `Transitive` means the scope was reached only by walking the cargo
/// reverse-dependency graph: a downstream crate depends on a changed one, but the
/// changed files do not fall in it. An additive change to a leaf crate cannot
/// break a transitive dependent, so this tag lets a consumer triage exit 6
/// (`direct` = real overlap, `transitive` = graph expansion) instead of
/// hand-diffing every collision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeCause {
    /// The changed files fall directly in this scope.
    Direct,
    /// Reached only via reverse-dependency expansion.
    Transitive,
}

impl ScopeCause {
    /// The canonical lowercase string (matches the JSON serialization).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeCause::Direct => "direct",
            ScopeCause::Transitive => "transitive",
        }
    }

    /// `Direct` dominates `Transitive`: a scope reached directly by *any* mapper
    /// is direct, even if another mapper reached it only transitively.
    fn merge(self, other: ScopeCause) -> ScopeCause {
        match (self, other) {
            (ScopeCause::Direct, _) | (_, ScopeCause::Direct) => ScopeCause::Direct,
            _ => ScopeCause::Transitive,
        }
    }
}

/// Insert `(scope, cause)` into `out`, applying the direct-wins merge rule.
pub fn merge_cause(out: &mut BTreeMap<String, ScopeCause>, scope: String, cause: ScopeCause) {
    out.entry(scope)
        .and_modify(|c| *c = c.merge(cause))
        .or_insert(cause);
}

/// Maps a set of changed files to the abstract scopes they affect, each tagged
/// with why it was affected (see [`ScopeCause`]).
pub trait AffectedSetMapper {
    /// Return the scopes affected by `changed_files`, keyed to their cause.
    fn map(&self, changed_files: &[String]) -> Result<BTreeMap<String, ScopeCause>>;
}

/// The scope mappers split by role (R10).
pub struct Mappers<'a> {
    /// File/pin-based mappers (path globs, external pins). Authoritative for
    /// under-declaration: what the branch *physically touched*.
    pub direct: &'a [&'a dyn AffectedSetMapper],
    /// Crate-graph reverse-dependency mappers. A non-failing impact signal that
    /// feeds collisions and the affected report, but never under-declaration —
    /// touching a foundational crate is not a scope escape.
    pub impact: &'a [&'a dyn AffectedSetMapper],
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
    fn map(&self, changed_files: &[String]) -> Result<BTreeMap<String, ScopeCause>> {
        // A path-glob match is always a direct file overlap.
        let mut out = BTreeMap::new();
        for (scope, set) in &self.scopes {
            if changed_files.iter().any(|f| set.is_match(f)) {
                out.insert(scope.clone(), ScopeCause::Direct);
            }
        }
        Ok(out)
    }
}

/// Maps a branch's diff to external/forked-dependency scopes. A scope fires when a
/// changed manifest line references its upstream `repo` (a pinned `rev` bump) or a
/// changed file matches one of its in-tree `paths` globs. All hits are `Direct` —
/// touching a fork pin is a real change, not reverse-dep expansion. Shells out to
/// `git` for the manifest diff, like [`BranchDiff::compute`].
pub struct ExternalScopeMapper {
    repo: PathBuf,
    base: String,
    branch: String,
    scopes: Vec<ExternalMatcher>,
}

/// A compiled external scope: its name, upstream repo token, and in-tree globs.
struct ExternalMatcher {
    name: String,
    repo: String,
    paths: GlobSet,
}

impl ExternalScopeMapper {
    /// Build from the diff range and the config's `[external_scopes]` table.
    pub fn new(
        repo: &Path,
        base: &str,
        branch: &str,
        external_scopes: &BTreeMap<String, ExternalScope>,
    ) -> Result<Self> {
        let mut scopes = Vec::new();
        for (name, ext) in external_scopes {
            let mut builder = GlobSetBuilder::new();
            for g in &ext.paths {
                builder.add(Glob::new(g).map_err(|e| {
                    Error::Invalid(format!(
                        "invalid glob `{g}` for external scope `{name}`: {e}"
                    ))
                })?);
            }
            let paths = builder.build().map_err(|e| {
                Error::Invalid(format!("building globset for external scope `{name}`: {e}"))
            })?;
            scopes.push(ExternalMatcher {
                name: name.clone(),
                repo: ext.repo.clone(),
                paths,
            });
        }
        Ok(Self {
            repo: repo.to_path_buf(),
            base: base.to_string(),
            branch: branch.to_string(),
            scopes,
        })
    }

    /// The diff of just the changed manifests, or empty if none changed.
    fn manifest_diff(&self, changed_files: &[String]) -> Result<String> {
        let manifests: Vec<&String> = changed_files.iter().filter(|f| is_manifest(f)).collect();
        if manifests.is_empty() {
            return Ok(String::new());
        }
        let range = format!("{}...{}", self.base, self.branch);
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.repo).args(["diff", &range, "--"]);
        for m in &manifests {
            cmd.arg(m);
        }
        let output = cmd
            .output()
            .map_err(|e| Error::Invalid(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Invalid(format!(
                "`git diff {range}` (manifests) failed: {}",
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl AffectedSetMapper for ExternalScopeMapper {
    fn map(&self, changed_files: &[String]) -> Result<BTreeMap<String, ScopeCause>> {
        if self.scopes.is_empty() {
            return Ok(BTreeMap::new());
        }
        let diff_text = self.manifest_diff(changed_files)?;
        Ok(external_hits(&diff_text, changed_files, &self.scopes))
    }
}

/// Pure matcher: a scope is hit when a changed manifest line (`+`/`-`) contains
/// its `repo` token, or a changed file matches one of its `paths` globs.
fn external_hits(
    diff_text: &str,
    changed_files: &[String],
    scopes: &[ExternalMatcher],
) -> BTreeMap<String, ScopeCause> {
    let changed_lines: Vec<&str> = diff_text
        .lines()
        .filter(|l| {
            (l.starts_with('+') || l.starts_with('-'))
                && !l.starts_with("+++")
                && !l.starts_with("---")
        })
        .collect();
    let mut out = BTreeMap::new();
    for s in scopes {
        let manifest_hit = !s.repo.is_empty() && changed_lines.iter().any(|l| l.contains(&s.repo));
        let path_hit = changed_files.iter().any(|f| s.paths.is_match(f));
        if manifest_hit || path_hit {
            out.insert(s.name.clone(), ScopeCause::Direct);
        }
    }
    out
}

/// Whether a repo-relative path is a cargo manifest (`Cargo.toml`) or lockfile.
fn is_manifest(file: &str) -> bool {
    file == "Cargo.toml"
        || file == "Cargo.lock"
        || file.ends_with("/Cargo.toml")
        || file.ends_with("/Cargo.lock")
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
    /// Why the collision fired: `direct` if any shared scope is a real file/crate
    /// overlap, else `transitive` (reverse-dependency expansion only — likely safe
    /// for an additive change). Lets a consumer auto-triage exit 6.
    pub cause: ScopeCause,
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
    /// Per-scope cause for every affected scope (`direct` vs `transitive`). Lets a
    /// consumer triage under-declarations and collisions without re-diffing.
    pub affected_causes: BTreeMap<String, ScopeCause>,
    /// Scopes the ticket declared.
    pub declared_scopes: Vec<String>,
    /// Affected scopes the ticket failed to declare (under-declaration).
    pub under_declared: Vec<String>,
    /// Overlaps with other open tickets.
    pub collisions: Vec<Collision>,
    /// True if the guard found any conflict.
    pub conflict: bool,
}

/// The ticket's declared file area: the globs of its declared scopes plus its
/// explicit `paths`. A changed file matching this set is "covered" — it cannot be
/// an under-declaration, regardless of which other scope's glob also matches it
/// (so an explicit `paths` entry suppresses an overlapping scope).
pub fn coverage_globset(config: &Config, ticket: &Ticket) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for scope in &ticket.scopes {
        if let Some(globs) = config.scopes.get(scope) {
            for g in globs {
                builder.add(Glob::new(g).map_err(|e| {
                    Error::Invalid(format!("invalid glob `{g}` for scope `{scope}`: {e}"))
                })?);
            }
        }
    }
    for p in &ticket.paths {
        builder.add(Glob::new(p).map_err(|e| {
            Error::Invalid(format!(
                "invalid path glob `{p}` on ticket `{}`: {e}",
                ticket.id
            ))
        })?);
    }
    builder
        .build()
        .map_err(|e| Error::Invalid(format!("building coverage globset: {e}")))
}

/// Evaluate the guard. Two distinct judgements, deliberately decoupled:
///
/// - **Under-declaration** (a scope *escape*) is file-authoritative: a changed
///   file outside the ticket's declared area (`declared_coverage`) is the only
///   thing that counts, and the scopes reported are those the `direct` (file/pin)
///   mappers attribute to those uncovered files. The crate-graph reverse-dependency
///   expansion (the `impact` mappers) NEVER drives this — touching a foundational
///   crate that everything depends on is not "the branch left its lane".
/// - **Collisions** with other open tickets use the full affected set (direct +
///   transitive impact), tagged by [`ScopeCause`] so a consumer can triage.
pub fn evaluate(
    target: &Ticket,
    all: &[Ticket],
    diff: BranchDiff,
    mappers: &Mappers,
    declared_coverage: &GlobSet,
) -> Result<GuardReport> {
    // Affected = what the branch physically touches (direct) plus its transitive
    // crate-graph impact (impact). Drives collisions and the informational report.
    let mut affected: BTreeMap<String, ScopeCause> = BTreeMap::new();
    for mapper in mappers.direct.iter().chain(mappers.impact.iter()) {
        for (scope, cause) in mapper.map(&diff.changed_files)? {
            merge_cause(&mut affected, scope, cause);
        }
    }

    let declared: BTreeSet<String> = target.scopes.iter().cloned().collect();

    // Under-declaration: only files outside the declared area, mapped to scopes by
    // the file/pin (direct) mappers. Impact scopes are excluded by construction.
    let uncovered: Vec<String> = diff
        .changed_files
        .iter()
        .filter(|f| !declared_coverage.is_match(f.as_str()))
        .cloned()
        .collect();
    let mut touched_uncovered: BTreeSet<String> = BTreeSet::new();
    for mapper in mappers.direct {
        for scope in mapper.map(&uncovered)?.into_keys() {
            touched_uncovered.insert(scope);
        }
    }
    let under_declared: Vec<String> = touched_uncovered
        .into_iter()
        .filter(|s| !declared.contains(s))
        .collect();

    let mut collisions = Vec::new();
    for other in all {
        if other.id == target.id || !other.status.is_open() {
            continue;
        }
        let other_declared: BTreeSet<String> = other.scopes.iter().cloned().collect();
        let shared: Vec<String> = affected
            .keys()
            .filter(|s| other_declared.contains(*s))
            .cloned()
            .collect();
        if !shared.is_empty() {
            // The collision is direct if any shared scope is a real overlap; only
            // a purely transitive overlap carries the `transitive` tag.
            let cause = if shared.iter().any(|s| affected[s] == ScopeCause::Direct) {
                ScopeCause::Direct
            } else {
                ScopeCause::Transitive
            };
            collisions.push(Collision {
                ticket: other.id.clone(),
                scopes: shared,
                cause,
            });
        }
    }

    let conflict = !under_declared.is_empty() || !collisions.is_empty();
    Ok(GuardReport {
        ticket: target.id.clone(),
        base: diff.base,
        branch: diff.branch,
        changed_files: diff.changed_files,
        affected_scopes: affected.keys().cloned().collect(),
        affected_causes: affected,
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

    fn cover(cfg: &Config, target: &Ticket) -> GlobSet {
        coverage_globset(cfg, target).unwrap()
    }

    #[test]
    fn path_glob_mapper_maps_files() {
        let cfg = config_with_scopes(&[("core", "core/**"), ("io", "io/**")]);
        let mapper = PathGlobMapper::new(&cfg).unwrap();
        let affected = mapper
            .map(&["core/src/lib.rs".to_string(), "docs/readme.md".to_string()])
            .unwrap();
        assert_eq!(affected.get("core"), Some(&ScopeCause::Direct));
        assert!(!affected.contains_key("io"));
    }

    #[test]
    fn under_declaration_is_a_conflict() {
        let cfg = config_with_scopes(&[("core", "core/**"), ("io", "io/**")]);
        let mapper = PathGlobMapper::new(&cfg).unwrap();
        let target = ticket("t", "in-progress", &["core"]);
        let all = vec![target.clone()];
        let cov = cover(&cfg, &target);
        let report = evaluate(
            &target,
            &all,
            diff(&["core/a.rs", "io/b.rs"]),
            &Mappers {
                direct: &[&mapper],
                impact: &[],
            },
            &cov,
        )
        .unwrap();
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
        let cov = cover(&cfg, &target);
        let report = evaluate(
            &target,
            &all,
            diff(&["core/a.rs"]),
            &Mappers {
                direct: &[&mapper],
                impact: &[],
            },
            &cov,
        )
        .unwrap();
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
        let cov = cover(&cfg, &target);
        let report = evaluate(
            &target,
            &all,
            diff(&["core/a.rs"]),
            &Mappers {
                direct: &[&mapper],
                impact: &[],
            },
            &cov,
        )
        .unwrap();
        assert!(!report.conflict);
    }

    /// A mapper with a fixed, cause-tagged output — exercises the cause logic in
    /// `evaluate` without a real cargo graph.
    struct StubMapper(BTreeMap<String, ScopeCause>);

    impl AffectedSetMapper for StubMapper {
        fn map(&self, _changed_files: &[String]) -> Result<BTreeMap<String, ScopeCause>> {
            Ok(self.0.clone())
        }
    }

    fn stub(pairs: &[(&str, ScopeCause)]) -> StubMapper {
        StubMapper(pairs.iter().map(|(s, c)| ((*s).to_string(), *c)).collect())
    }

    #[test]
    fn collision_cause_is_transitive_when_only_transitive_shared() {
        let target = ticket("t", "in-progress", &["core"]);
        let other = ticket("u", "in-progress", &["dep"]);
        let all = vec![target.clone(), other];
        let mapper = stub(&[
            ("core", ScopeCause::Direct),
            ("dep", ScopeCause::Transitive),
        ]);
        let cov = cover(&Config::default(), &target);
        let report = evaluate(
            &target,
            &all,
            diff(&["core/a.rs"]),
            &Mappers {
                direct: &[],
                impact: &[&mapper],
            },
            &cov,
        )
        .unwrap();
        let c = &report.collisions[0];
        assert_eq!(c.ticket, "u");
        assert_eq!(c.scopes, vec!["dep"]);
        assert_eq!(c.cause, ScopeCause::Transitive);
        assert_eq!(
            report.affected_causes.get("dep"),
            Some(&ScopeCause::Transitive)
        );
        assert_eq!(
            report.affected_causes.get("core"),
            Some(&ScopeCause::Direct)
        );
    }

    #[test]
    fn collision_cause_is_direct_when_a_direct_scope_is_shared() {
        let target = ticket("t", "in-progress", &["core"]);
        // The other ticket shares both a direct and a transitive scope: direct wins.
        let other = ticket("u", "in-progress", &["core", "dep"]);
        let all = vec![target.clone(), other];
        let mapper = stub(&[
            ("core", ScopeCause::Direct),
            ("dep", ScopeCause::Transitive),
        ]);
        let cov = cover(&Config::default(), &target);
        let report = evaluate(
            &target,
            &all,
            diff(&["core/a.rs"]),
            &Mappers {
                direct: &[],
                impact: &[&mapper],
            },
            &cov,
        )
        .unwrap();
        assert_eq!(report.collisions[0].cause, ScopeCause::Direct);
    }

    #[test]
    fn direct_cause_wins_across_mappers() {
        let target = ticket("t", "in-progress", &["other-scope"]);
        let all = vec![target.clone()];
        let m1 = stub(&[("x", ScopeCause::Transitive)]);
        let m2 = stub(&[("x", ScopeCause::Direct)]);
        let cov = cover(&Config::default(), &target);
        let report = evaluate(
            &target,
            &all,
            diff(&["a"]),
            &Mappers {
                direct: &[],
                impact: &[&m1, &m2],
            },
            &cov,
        )
        .unwrap();
        assert_eq!(report.affected_causes.get("x"), Some(&ScopeCause::Direct));
    }

    fn ext_matcher(name: &str, repo: &str, globs: &[&str]) -> ExternalMatcher {
        let mut b = GlobSetBuilder::new();
        for g in globs {
            b.add(Glob::new(g).unwrap());
        }
        ExternalMatcher {
            name: name.to_string(),
            repo: repo.to_string(),
            paths: b.build().unwrap(),
        }
    }

    #[test]
    fn external_hits_flags_a_manifest_rev_bump() {
        let scopes = vec![ext_matcher("sqlparser-fork", "tomsanbear/sqlparser", &[])];
        let diff = "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1 +1 @@\n\
                    -sqlparser = { git = \"https://github.com/tomsanbear/sqlparser\", rev = \"aaa\" }\n\
                    +sqlparser = { git = \"https://github.com/tomsanbear/sqlparser\", rev = \"bbb\" }\n";
        let hits = external_hits(diff, &["Cargo.toml".to_string()], &scopes);
        assert_eq!(hits.get("sqlparser-fork"), Some(&ScopeCause::Direct));
    }

    #[test]
    fn external_hits_flags_an_in_tree_path() {
        let scopes = vec![ext_matcher("vendored", "", &["vendor/sqlparser/**"])];
        let hits = external_hits("", &["vendor/sqlparser/src/lib.rs".to_string()], &scopes);
        assert_eq!(hits.get("vendored"), Some(&ScopeCause::Direct));
    }

    #[test]
    fn external_hits_ignores_context_lines_and_unrelated_paths() {
        let scopes = vec![ext_matcher(
            "sqlparser-fork",
            "tomsanbear/sqlparser",
            &["vendor/**"],
        )];
        // A context line (leading space) mentioning the repo must not fire.
        let diff = " sqlparser = { git = \"https://github.com/tomsanbear/sqlparser\" }\n";
        let hits = external_hits(diff, &["src/main.rs".to_string()], &scopes);
        assert!(hits.is_empty());
    }
}
