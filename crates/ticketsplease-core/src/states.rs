//! The workflow state registry: the set of lifecycle states a repo recognizes and,
//! for each, the engine **category** that drives scheduling, guarding, and rollup.
//!
//! State *names* are the repo's to choose (`todo`, `qa`, `staged`, …); the *category*
//! is a fixed, engine-owned enum the scheduler/guard/rollup branch on. Every state must
//! declare one — that is the whole trick that lets the engine reason about states it has
//! never seen. With no `[workflow]` config a repo gets [`StateRegistry::builtin`] (the
//! historical six plus `closed`); config replaces that set (see `Config::state_registry`).

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The engine-owned category every state maps to. The scheduler/guard/rollup branch on
/// this, never on the state name — so a custom state behaves correctly the moment it
/// declares its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Eligible for `ready`/`next`/scheduling (e.g. `todo`, `ready`).
    Dispatchable,
    /// Actively worked; occupies its declared code scopes, so the guard fires collisions
    /// on it (e.g. `in-progress`, `review`).
    Open,
    /// Excluded from dispatch but holds no scope lock and is not finished (e.g. `blocked`).
    Parked,
    /// Finished; excluded from scheduling. Whether it *satisfies dependents* is a separate
    /// bit (`satisfies_dependents`) — `done` does, `closed` does not.
    Terminal,
}

impl Category {
    /// The canonical lowercase string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Dispatchable => "dispatchable",
            Category::Open => "open",
            Category::Parked => "parked",
            Category::Terminal => "terminal",
        }
    }

    /// Lifecycle sort rank (dispatchable → open → parked → terminal).
    fn rank(self) -> u8 {
        match self {
            Category::Dispatchable => 0,
            Category::Open => 1,
            Category::Parked => 2,
            Category::Terminal => 3,
        }
    }
}

impl FromStr for Category {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "dispatchable" => Category::Dispatchable,
            "open" => Category::Open,
            "parked" => Category::Parked,
            "terminal" => Category::Terminal,
            _ => {
                return Err(Error::Invalid(format!(
                    "unknown state category `{s}` (expected dispatchable|open|parked|terminal)"
                )))
            }
        })
    }
}

/// The resolved behavioural class of a state — its category plus the `satisfies_dependents`
/// bit (meaningful only for `terminal`) and a lifecycle sort order. Stamped onto each
/// [`Ticket`](crate::Ticket) at load so the scheduling predicates need no registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateClass {
    /// The engine category.
    pub category: Category,
    /// Whether reaching this (terminal) state satisfies a dependent's dependency. Always
    /// `false` for non-terminal states.
    pub satisfies_dependents: bool,
    /// Lifecycle sort order (built-ins keep their classic order; custom states sort after).
    pub order: u32,
}

impl StateClass {
    /// The conservative class for a state the registry does not know (a typo, or a ticket
    /// left in a since-removed state): inert — not dispatchable, not scope-occupying, not
    /// terminal — so it is never scheduled onto and never silently satisfies a dependency.
    /// `lint`/`doctor` flag the unknown state separately.
    pub const UNKNOWN: StateClass = StateClass {
        category: Category::Parked,
        satisfies_dependents: false,
        order: u32::MAX,
    };

    /// Eligible for dispatch (todo/ready-like).
    #[must_use]
    pub fn is_dispatchable(self) -> bool {
        self.category == Category::Dispatchable
    }

    /// Actively worked — occupies its scopes for the guard (in-progress/review-like).
    #[must_use]
    pub fn is_open(self) -> bool {
        self.category == Category::Open
    }

    /// Finished for scheduling (done/closed-like): excluded from the ready queue, drops
    /// its claim.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        self.category == Category::Terminal
    }

    /// Reaching this state satisfies a dependent's dependency — a terminal state that
    /// completed (`done`), not one that was abandoned (`closed`).
    #[must_use]
    pub fn completes_dependencies(self) -> bool {
        self.category == Category::Terminal && self.satisfies_dependents
    }

    /// A terminal state that does *not* satisfy dependents (a "dropped"/"cancelled" state,
    /// e.g. `closed`). Such states bear the `closed_reason`/`closed_note` resolution.
    #[must_use]
    pub fn is_dropped(self) -> bool {
        self.category == Category::Terminal && !self.satisfies_dependents
    }
}

/// The raw per-state config (`[workflow.states.<name>]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDef {
    /// The engine category (required — the semantic contract).
    pub category: Category,
    /// Whether a terminal state satisfies dependents. Defaults to `true`; only meaningful
    /// for `terminal` states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satisfies_dependents: Option<bool>,
}

/// The resolved set of workflow states a repo recognizes.
#[derive(Debug, Clone)]
pub struct StateRegistry {
    /// name -> resolved class, sorted by name (lifecycle order via `StateClass::order`).
    states: BTreeMap<String, StateClass>,
}

impl StateRegistry {
    /// The built-in default registry: the historical `todo`/`ready`/`in-progress`/
    /// `blocked`/`review`/`done` plus `closed`, in classic lifecycle order. Used when a
    /// repo declares no `[workflow.states]`.
    #[must_use]
    pub fn builtin() -> Self {
        let sat = |b| Some(b);
        let defs = [
            ("todo", Category::Dispatchable, None, 0),
            ("ready", Category::Dispatchable, None, 1),
            ("in-progress", Category::Open, None, 2),
            ("blocked", Category::Parked, None, 3),
            ("review", Category::Open, None, 4),
            ("done", Category::Terminal, sat(true), 5),
            ("closed", Category::Terminal, sat(false), 6),
        ];
        let states = defs
            .into_iter()
            .map(|(name, category, satisfies, order)| {
                (
                    name.to_string(),
                    StateClass {
                        category,
                        satisfies_dependents: satisfies.unwrap_or(true)
                            && category == Category::Terminal,
                        order,
                    },
                )
            })
            .collect();
        Self { states }
    }

    /// Build a registry from a config `[workflow.states]` table. Custom states sort after
    /// the built-ins (by name). Validated by [`validate`](Self::validate).
    #[must_use]
    pub fn from_defs(defs: &BTreeMap<String, StateDef>) -> Self {
        let builtin_order: BTreeMap<&str, u32> = [
            ("todo", 0),
            ("ready", 1),
            ("in-progress", 2),
            ("blocked", 3),
            ("review", 4),
            ("done", 5),
            ("closed", 6),
        ]
        .into_iter()
        .collect();
        let states = defs
            .iter()
            .map(|(name, def)| {
                let category = def.category;
                // State names are canonicalized to lowercase (matching `todo`/`in-progress`
                // conventions), so lookups are case-insensitive.
                let key = name.trim().to_ascii_lowercase();
                let order = builtin_order.get(key.as_str()).copied().unwrap_or(100);
                (
                    key,
                    StateClass {
                        category,
                        // The bit only means anything for terminal states; force false
                        // elsewhere so `completes_dependencies` can't be tricked.
                        satisfies_dependents: category == Category::Terminal
                            && def.satisfies_dependents.unwrap_or(true),
                        order,
                    },
                )
            })
            .collect();
        Self { states }
    }

    /// The class of `name` (case-insensitive), or [`StateClass::UNKNOWN`] if the registry
    /// does not define it.
    #[must_use]
    pub fn class(&self, name: &str) -> StateClass {
        self.states
            .get(name.trim().to_ascii_lowercase().as_str())
            .copied()
            .unwrap_or(StateClass::UNKNOWN)
    }

    /// Whether `name` is a defined state (case-insensitive).
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.states
            .contains_key(name.trim().to_ascii_lowercase().as_str())
    }

    /// State names sorted in lifecycle order (category, then declared order, then name).
    #[must_use]
    pub fn ordered_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.states.keys().map(String::as_str).collect();
        names.sort_by_key(|n| {
            let c = self.states[*n];
            (c.category.rank(), c.order, *n)
        });
        names
    }

    /// The state a fresh claim moves a ticket into — the primary `open` state. Prefers a
    /// state literally named `in-progress`; otherwise the first `open` state in lifecycle
    /// order. `None` if the workflow has no open state (claiming is then unavailable).
    #[must_use]
    pub fn primary_open(&self) -> Option<&str> {
        if self.class("in-progress").is_open() {
            return Some("in-progress");
        }
        self.ordered_names()
            .into_iter()
            .find(|n| self.class(n).is_open())
    }

    /// The primary "dropped" state `close` moves a ticket into — a terminal state that does
    /// not satisfy dependents. Prefers `closed`; otherwise the first such state. `None` if
    /// the workflow has no dropped state.
    #[must_use]
    pub fn primary_dropped(&self) -> Option<&str> {
        if self.class("closed").is_dropped() {
            return Some("closed");
        }
        self.ordered_names()
            .into_iter()
            .find(|n| self.class(n).is_dropped())
    }

    /// The default state for a newly-created ticket: `todo` if defined, else the first
    /// dispatchable state, else the first state in order.
    #[must_use]
    pub fn default_state(&self) -> &str {
        if self.class("todo").is_dispatchable() {
            return "todo";
        }
        let ordered = self.ordered_names();
        ordered
            .iter()
            .copied()
            .find(|n| self.class(n).is_dispatchable())
            .or_else(|| ordered.first().copied())
            .unwrap_or("todo")
    }

    /// Validate category coverage: a workflow with no dispatchable state can never start
    /// work, and one with no terminal state can never finish it. Both are errors. (A
    /// missing `open` state is only a `doctor` warning — a repo may legitimately not gate
    /// on the guard.)
    pub fn validate(&self) -> Result<()> {
        if self.states.is_empty() {
            return Err(Error::Invalid(
                "[workflow.states] is empty; define at least one dispatchable and one terminal state"
                    .into(),
            ));
        }
        if !self.states.values().any(|c| c.is_dispatchable()) {
            return Err(Error::Invalid(
                "workflow has no `dispatchable` state — nothing could ever be picked up".into(),
            ));
        }
        if !self.states.values().any(|c| c.is_terminal()) {
            return Err(Error::Invalid(
                "workflow has no `terminal` state — nothing could ever be finished".into(),
            ));
        }
        Ok(())
    }

    /// Whether the workflow defines at least one `open` state (guardable work).
    #[must_use]
    pub fn has_open_state(&self) -> bool {
        self.states.values().any(|c| c.is_open())
    }

    /// Iterate (name, class) in lifecycle order — for `tkt states` and rollup.
    pub fn iter_ordered(&self) -> impl Iterator<Item = (&str, StateClass)> {
        self.ordered_names()
            .into_iter()
            .map(move |n| (n, self.class(n)))
    }
}

impl Default for StateRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_expected_categories_and_roles() {
        let r = StateRegistry::builtin();
        assert!(r.class("todo").is_dispatchable());
        assert!(r.class("in-progress").is_open());
        assert!(r.class("blocked").category == Category::Parked);
        assert!(r.class("done").completes_dependencies());
        assert!(r.class("closed").is_terminal());
        assert!(!r.class("closed").completes_dependencies());
        assert!(r.class("closed").is_dropped());
        assert_eq!(r.primary_open(), Some("in-progress"));
        assert_eq!(r.primary_dropped(), Some("closed"));
        assert_eq!(r.default_state(), "todo");
        // Lifecycle order groups by category (dispatchable → open → parked → terminal),
        // then declared order within a category — so both open states precede `blocked`.
        assert_eq!(
            r.ordered_names(),
            vec![
                "todo",
                "ready",
                "in-progress",
                "review",
                "blocked",
                "done",
                "closed"
            ]
        );
        r.validate().unwrap();
        assert!(r.has_open_state());
    }

    #[test]
    fn unknown_state_is_inert() {
        let r = StateRegistry::builtin();
        let c = r.class("qa");
        assert!(!r.contains("qa"));
        assert!(!c.is_dispatchable() && !c.is_open() && !c.is_terminal());
        assert!(!c.completes_dependencies());
    }

    #[test]
    fn custom_states_resolve_and_sort_after_builtins() {
        let defs: BTreeMap<String, StateDef> = [
            (
                "todo".to_string(),
                StateDef {
                    category: Category::Dispatchable,
                    satisfies_dependents: None,
                },
            ),
            (
                "qa".to_string(),
                StateDef {
                    category: Category::Open,
                    satisfies_dependents: None,
                },
            ),
            (
                "shipped".to_string(),
                StateDef {
                    category: Category::Terminal,
                    satisfies_dependents: Some(true),
                },
            ),
            (
                "wontfix".to_string(),
                StateDef {
                    category: Category::Terminal,
                    satisfies_dependents: Some(false),
                },
            ),
        ]
        .into_iter()
        .collect();
        let r = StateRegistry::from_defs(&defs);
        assert!(r.class("qa").is_open());
        assert!(r.class("shipped").completes_dependencies());
        assert!(r.class("wontfix").is_dropped());
        // No `in-progress`/`closed` here, so roles fall back to the custom states.
        assert_eq!(r.primary_open(), Some("qa"));
        assert_eq!(r.primary_dropped(), Some("wontfix"));
        r.validate().unwrap();
        // `satisfies_dependents` on a non-terminal state is ignored (forced false).
        let mut bad = defs.clone();
        bad.insert(
            "weird".into(),
            StateDef {
                category: Category::Dispatchable,
                satisfies_dependents: Some(true),
            },
        );
        assert!(!StateRegistry::from_defs(&bad)
            .class("weird")
            .completes_dependencies());
    }

    #[test]
    fn validate_rejects_missing_coverage() {
        let only_dispatch: BTreeMap<String, StateDef> = [(
            "todo".to_string(),
            StateDef {
                category: Category::Dispatchable,
                satisfies_dependents: None,
            },
        )]
        .into_iter()
        .collect();
        assert!(StateRegistry::from_defs(&only_dispatch).validate().is_err());

        let only_terminal: BTreeMap<String, StateDef> = [(
            "done".to_string(),
            StateDef {
                category: Category::Terminal,
                satisfies_dependents: Some(true),
            },
        )]
        .into_iter()
        .collect();
        assert!(StateRegistry::from_defs(&only_terminal).validate().is_err());
    }
}
