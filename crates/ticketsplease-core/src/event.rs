//! The activity event log — a `refs/ticketsplease/events/<id>` namespace, one
//! ref per event pointing at a JSON blob.
//!
//! This is the live coordination channel for an orchestrator driving many worker
//! subagents. It is the claim mutex generalized: events live entirely in `.git`
//! (never the working tree), so they are visible across worktrees and across a
//! single shared clone the instant they are written — no commit, no push, no
//! merge. Per-ref atomic creation makes concurrent emit conflict-free. The id is
//! sortable, so a consumer tails the log with a `--since <id>` cursor and never
//! misses a transition the way polling current state can.

use serde::{Deserialize, Serialize};

/// One activity event (a comment, a status change, a claim, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Sortable unique id (orders the log; also the cursor value).
    pub id: String,
    /// The ticket this event concerns.
    pub ticket: String,
    /// Event kind, e.g. `comment`, `status`, `claim`, `release`.
    pub kind: String,
    /// Actor, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Emission time (epoch seconds).
    pub at: u64,
    /// Kind-specific payload (e.g. `{ "status": "review" }` or a comment ref).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}
