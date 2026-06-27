//! Schema-level linting of tickets. Link validation and cycle detection live in
//! the scheduling layer (milestone M3) and reuse these diagnostics.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

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
    /// `duplicate-id` | `missing-dep` | `cycle`.
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
}

/// Run schema lint across all ticket files. Returns findings (possibly empty):
/// parse failures, id/filename mismatches, and duplicate ids.
pub fn lint(store: &Store) -> Result<Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
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
