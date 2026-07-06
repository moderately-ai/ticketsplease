//! Schema-level linting of tickets. Link validation and cycle detection live in
//! the scheduling layer (milestone M3) and reuse these diagnostics.

use std::collections::BTreeMap;
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
    /// `unknown-scope` | `unknown-scope-policy` | `scope-mode-conflict` | `duplicate-id`
    /// | `unknown-state` | `state-coverage` | `unknown-transition-state` |
    /// `dead-end-nonterminal` | `stale-resolution` | `missing-dep` | `missing-related` |
    /// `cycle`.
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
}

/// Run schema lint across all ticket files. Returns findings (possibly empty):
/// parse failures, id/filename mismatches, and duplicate ids.
pub fn lint(store: &Store) -> Result<Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let registry = store.config.state_registry();
    // A workflow with no dispatchable or no terminal state is unusable (nothing can be
    // started or finished) — surface it once against the config.
    if let Err(e) = registry.validate() {
        diags.push(Diagnostic {
            file: CONFIG_FILE.to_string(),
            id: None,
            code: "state-coverage",
            message: e.message(),
        });
    }
    // Validate the transition graph when one is declared: every edge must name a defined
    // state, and — only under enforcement — a non-terminal state needs a way out (else a
    // ticket there can never advance).
    let wf = &store.config.workflow;
    if !wf.transitions.is_empty() {
        for (from, targets) in &wf.transitions {
            if from != "*" && !registry.contains(from) {
                diags.push(Diagnostic {
                    file: CONFIG_FILE.to_string(),
                    id: None,
                    code: "unknown-transition-state",
                    message: format!(
                        "[workflow.transitions] source `{from}` is not a defined state"
                    ),
                });
            }
            for to in targets {
                if !registry.contains(to) {
                    diags.push(Diagnostic {
                        file: CONFIG_FILE.to_string(),
                        id: None,
                        code: "unknown-transition-state",
                        message: format!(
                            "[workflow.transitions] `{from}` -> `{to}` targets an undefined state"
                        ),
                    });
                }
            }
        }
        if wf.enforce_transitions {
            let wildcard_out = wf.transitions.get("*").is_some_and(|t| !t.is_empty());
            for name in registry.ordered_names() {
                let has_out =
                    wf.transitions.get(name).is_some_and(|t| !t.is_empty()) || wildcard_out;
                if !registry.class(name).is_terminal() && !has_out {
                    diags.push(Diagnostic {
                        file: CONFIG_FILE.to_string(),
                        id: None,
                        code: "dead-end-nonterminal",
                        message: format!(
                            "state `{name}` is non-terminal but has no outbound transition — a \
                             ticket there can never advance"
                        ),
                    });
                }
            }
        }
    }
    // A ticket declaring an undefined scope (a typo) would otherwise only surface as a
    // baffling later guard CONFLICT, so flag it like a dangling dep. `create`/`set`
    // reuse the same vocabulary to reject a bad scope at write time.
    let defined_scopes = store.config.defined_scopes();
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
                    for scope in ticket.scopes.iter().chain(&ticket.shared_scopes) {
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
                // A scope can be claimed exclusively or shared, not both — the mode
                // would be ambiguous for conflict detection.
                for scope in &ticket.scopes {
                    if ticket.shared_scopes.contains(scope) {
                        diags.push(Diagnostic {
                            file: file.clone(),
                            id: Some(ticket.id.clone()),
                            code: "scope-mode-conflict",
                            message: format!(
                                "scope `{scope}` is declared both exclusive (`scopes`) and \
                                 shared (`shared_scopes`)"
                            ),
                        });
                    }
                }
                // A state the registry doesn't define (a typo, or a state removed from
                // config that a ticket still occupies). The engine treats it as inert.
                if !registry.contains(&ticket.status) {
                    diags.push(Diagnostic {
                        file: file.clone(),
                        id: Some(ticket.id.clone()),
                        code: "unknown-state",
                        message: format!(
                            "status `{}` is not a defined workflow state",
                            ticket.status
                        ),
                    });
                }
                // Resolution metadata is only meaningful on a *dropped* (terminal,
                // non-satisfying) state like `closed`. If it lingers elsewhere (e.g. a
                // hand-edit that flipped status but left the reason), the "why"
                // contradicts the live status.
                if !registry.class(&ticket.status).is_dropped()
                    && (ticket.closed_reason.is_some() || ticket.closed_note.is_some())
                {
                    diags.push(Diagnostic {
                        file: file.clone(),
                        id: Some(ticket.id.clone()),
                        code: "stale-resolution",
                        message: format!(
                            "has closed_reason/closed_note but status `{}` is not a dropped (closed) state",
                            ticket.status
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
    // `[scope_policy]` keys must name real scopes, or a weight typo silently does nothing.
    if !defined_scopes.is_empty() {
        for scope in store.config.scope_policy.keys() {
            if !defined_scopes.contains(scope.as_str()) {
                diags.push(Diagnostic {
                    file: CONFIG_FILE.to_string(),
                    id: None,
                    code: "unknown-scope-policy",
                    message: format!("[scope_policy] entry `{scope}` is not a defined scope"),
                });
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
