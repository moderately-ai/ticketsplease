//! Schema-level linting of tickets. Link validation and cycle detection live in
//! the scheduling layer (milestone M3) and reuse these diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::config::CONFIG_FILE;
use crate::error::{Error, Result};
use crate::store::Store;
use crate::ticket::Ticket;

/// A single lint finding.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// Ticket file, relative to the repo root.
    pub file: String,
    /// The ticket id, when parseable.
    pub id: Option<String>,
    /// A stable machine-readable kind: `parse` | `id-mismatch` | `bad-id` |
    /// `unknown-scope` | `duplicate-id` | `missing-dep` | `missing-related` | `cycle`.
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
}

/// Run schema lint across all ticket files. Returns findings (possibly empty):
/// parse failures, id/filename mismatches, and duplicate ids.
pub fn lint(store: &Store) -> Result<Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    // A scope is "defined" if it has a glob mapping, an owning crate, or an external
    // descriptor. A ticket declaring an undefined scope (a typo) would otherwise only
    // surface as a baffling later guard CONFLICT, so flag it like a dangling dep.
    let defined_scopes: BTreeSet<&str> = store
        .config
        .scopes
        .keys()
        .chain(store.config.scope_crates.keys())
        .chain(store.config.external_scopes.keys())
        .map(String::as_str)
        .collect();
    for path in store.ticket_files()? {
        let file = rel(&store.repo_root, &path);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let raw = std::fs::read_to_string(&path).map_err(Error::Io)?;
        match Ticket::parse(&raw) {
            Err(e) => diags.push(Diagnostic {
                file,
                id: None,
                code: "parse",
                message: e.message(),
            }),
            Ok(ticket) => {
                if ticket.id != stem {
                    diags.push(Diagnostic {
                        file: file.clone(),
                        id: Some(ticket.id.clone()),
                        code: "id-mismatch",
                        message: format!(
                            "id `{}` does not match filename stem `{stem}`",
                            ticket.id
                        ),
                    });
                }
                if crate::store::validate_slug(&ticket.id).is_err() {
                    diags.push(Diagnostic {
                        file: file.clone(),
                        id: Some(ticket.id.clone()),
                        code: "bad-id",
                        message: format!(
                            "id `{}` is not a valid slug (lowercase letters, digits, single hyphens)",
                            ticket.id
                        ),
                    });
                }
                // Only enforce the scope vocabulary once one exists: with no scopes
                // configured at all the project is not using the scope system, so
                // there is nothing to typo against (and no false alarms on a fresh repo).
                if !defined_scopes.is_empty() {
                    for scope in &ticket.scopes {
                        if !defined_scopes.contains(scope.as_str()) {
                            diags.push(Diagnostic {
                                file: file.clone(),
                                id: Some(ticket.id.clone()),
                                code: "unknown-scope",
                                message: format!(
                                    "declares scope `{scope}` not defined in {CONFIG_FILE} \
                                     ([scopes], [scope_crates], or [external_scopes])"
                                ),
                            });
                        }
                    }
                }
                if let Some(prev) = seen.insert(ticket.id.clone(), file.clone()) {
                    diags.push(Diagnostic {
                        file,
                        id: Some(ticket.id.clone()),
                        code: "duplicate-id",
                        message: format!("duplicate id `{}` (also defined in {prev})", ticket.id),
                    });
                }
            }
        }
    }
    Ok(diags)
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
