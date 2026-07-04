---
id: config-custom-states
title: "Config-driven custom states, each pinned to an engine category"
status: todo
priority: p2
dependencies: [closed-status-resolution]
related: [state-transition-workflows]
scopes: [core, cli, skill, docs]
paths: []
tags: [state-model, feature]
---
## Goal

Let a repo define its own workflow states in `ticketsplease.toml` (e.g. `design-review`, `qa`, `staged`, `awaiting-deploy`) instead of the fixed six the binary ships. Replace the hard-coded `Status` enum with a config-backed **state registry** — but keep the scheduler, guard, and rollup working over states they have never heard of by requiring every state to declare an **engine-owned category**. Names are the repo's to choose; the category is the semantic contract the engine reasons about.

## Gap

`Status` is a closed 6-variant enum (`ticketsplease-core/src/ticket.rs:19`). Its three (soon four, after [[closed-status-resolution]]) behavioural distinctions — dispatchable, open/occupies-scope, terminal, satisfies-dependents — are baked into `impl Status` and consulted at ~20 sites. A repo that runs a QA gate or a design-review step has no way to express it without forking the binary. Every state field, filter, board column, and rollup bucket is likewise pinned to those six names.

## Design decisions (prior art)

Surveyed Jira, Linear, Azure DevOps, Shortcut, GitLab, GitHub Projects, YouTrack, Trello. The convergence is striking and drives the decisions below.

- **`category` is REQUIRED on every state, from a closed enum the engine owns.** This is the single most-validated decision in the whole space: Jira ("all statuses, even custom ones, must belong to one of three status categories"), Linear (every state carries a `type` from a fixed set), Azure ("state categories determine how planning tools treat each state"), Shortcut, and GitLab's brand-new Status field all force it. The reason is exactly our problem: the engine must answer "is this startable / active / finished?" *generically* across unknown state names, and it can only do that off a category. Free-form uncategorized states are what GitLab is actively *abandoning* (labels "informal and scattered… without standardization"). So: state **names are user-defined; categories are not**.
- **Category set = `dispatchable | open | parked | terminal`** (engine-owned, closed). This is the current engine's behaviour faithfully named. The brief recommended three (`dispatchable/open/terminal`), but our `blocked` state is a genuine fourth tuple — not dispatchable (excluded from `ready`/`next`), not open (the `guard` does *not* treat it as occupying a scope, `guard.rs:462`), not terminal. That "parked" behaviour is a real branch the engine already takes, so it earns a category (the brief's own rule: "add a category only when the engine must branch on it" — here it must).
  - `dispatchable` — eligible for `ready`/`next`/scheduling (today: todo, ready).
  - `open` — actively worked; occupies its declared code scopes, so the guard fires collisions on it (today: in-progress, review).
  - `parked` — excluded from dispatch but holds no scope lock and is not finished (today: blocked).
  - `terminal` — excluded from scheduling; finished (today: done, and closed from #1).
- **Split "finished" with a `satisfies_dependents` bool on terminal states** (default `true`). Every serious tracker separates Completed from Canceled/Removed for exactly this reason — a completed blocker unblocks dependents; a cancelled one is terminal but must not. This is the config form of #1's `done`-vs-`closed` split: `done` → `terminal` + `satisfies_dependents = true`; `closed` → `terminal` + `false`. The orphaned-dependent detection from #1 keys on this bit generically.
- **Category-coverage validation** (steal from Azure's "maintain category coverage"): reject a config with **no `dispatchable`** state or **no `terminal`** state (the board would be un-startable or un-finishable); warn if there is no `open` state (nothing the guard can protect). Optionally enforce Azure's "one primary done" for unambiguous `% done`.
- **States are repo-local; the category is the stable contract.** Defined in *this repo's* toml, never a global/instance-wide registry (Jira's global status table is the root of its cross-project deletion hazards). Because the engine reasons on the category, **renaming a state while keeping its category never breaks the engine** — make that an explicit guarantee, and persist/reason on category with the display name treated as mutable.
- **Zero-config back-compat.** When `[workflow.states]` is absent, the engine seeds the current built-ins (todo, ready, in-progress, blocked, review, done + closed) with their categories, so every existing repo keeps working untouched. Presence of `[workflow.states]` makes it the authoritative set (subject to coverage validation); `tkt init` can scaffold the defaults as a starting point.

## Work

**Config (`ticketsplease-core/src/config.rs`)** — mirror the existing `[scope_policy]` → `ScopePolicy` pattern.
- `Workflow { states: BTreeMap<String, StateDef>, enforce_transitions: bool, transitions: BTreeMap<String, Vec<String>> }` under a new `[workflow]` table. (`enforce_transitions` + `transitions` are *defined* here but *consumed* by [[state-transition-workflows]]; carrying them in the struct now avoids a second config migration.)
- `StateDef { category: Category, satisfies_dependents: Option<bool> }`; `Category` enum (`dispatchable | open | parked | terminal`, serde kebab/lower).
- A `StateRegistry` built from `Workflow` (or the built-in defaults when absent) exposing the predicates the engine needs: `category(&str) -> Option<Category>`, `is_dispatchable`, `is_open`, `is_parked`, `is_terminal`, `completes_dependencies` (`terminal` ∧ `satisfies_dependents`). Coverage + validation live here.

**Core state model (`ticketsplease-core/src/ticket.rs`)** — the load-bearing refactor.
- `Ticket.status` becomes a validated state **string** (not the closed enum). `Ticket::parse` has no config, so it stores the raw state and defers category questions to the registry; strict validation ("is this a known state?") happens at the layer that has config (`Store`/commands), like scope validation does today.
- Move the four predicates off `impl Status` onto the registry (they were introduced in #1 as `is_terminal`/`completes_dependencies`; generalize the rest). Retire the enum. Update the ~20 call sites — mechanical, since most already hold `store`/`config`: `schedule.rs` (`ready`, dep-satisfaction, dispatchable filter — thread the registry into the pure schedule fns), `claim.rs` (claimable set, unmet-deps, heal), `commands.rs` (`--hide-done`→hide-terminal, rollup counts/blocked/orphan, doctor, claims, watch), `query.rs` (status-filter validation against the registry).
- Rollup's hard-coded lifecycle-order array (`commands.rs:1232`) can no longer list six variants: order buckets by category (dispatchable → open → parked → terminal) then config declaration order.

**Validation, migration, discoverability**
- `lint.rs` / `doctor`: new codes — `unknown-state` (a ticket uses a state absent from the registry), `category-coverage` (config missing a dispatchable or terminal state), `unknown-satisfies-dependents` (the bool set on a non-terminal state). Extend the existing scope-vocabulary-style validation.
- `migrate.rs`: bump `schema_version` to 2. A migration step **validates existing tickets against the new registry** and refuses to strand tickets in an undefined state — require an explicit `--remap old=new` (or a `[migrate]` block) when a config change removes/renames a state that live tickets occupy. Never silently orphan (the universal rule across Shortcut/Aha!/Azure). Prefer hide-don't-delete for built-ins.
- `create`/`set --status` validate the value against the registry (typo → exit 3), replacing the old enum parse.
- New `tkt states` command (small): list configured states with their category + `satisfies_dependents`, for discoverability (the config analogue of `tkt guide`).

**Docs**: README `status:` line (`:65`) and `## Status` section (`:132`); skill `SKILL.md`/`references/*` lifecycle prose — document `[workflow.states]`, the four categories, coverage rules, the rename-is-safe guarantee, and zero-config back-compat.

## Acceptance

- With no `[workflow]` config, behaviour is byte-for-byte identical to today plus #1 (the six built-ins + closed, same categories) — no existing repo needs to change anything.
- Adding `[workflow.states.qa] category = "open"` makes `qa` a first-class state: `set X --status qa` is accepted, `X` occupies its scopes for the guard, it is excluded from `ready`, and it appears in `rollup` under the open group. An unknown state (`--status qaa`) exits 3.
- A terminal state with `satisfies_dependents = true` unblocks dependents; one with `false` triggers the orphaned-dependent path from #1 — proving the config model reproduces the built-in `done`/`closed` behaviours.
- A config with no dispatchable state, or no terminal state, is rejected by `doctor`/load with a `category-coverage` diagnostic. `satisfies_dependents` on a non-terminal state is flagged.
- Renaming a state in config (same category) while remapping live tickets (`--remap old=new`) leaves the board fully functional; removing a state that tickets still occupy without a remap is refused, not silently applied.
- `tkt states` lists the effective registry.

## Out of scope

- Enforcing state→state transitions — that is [[state-transition-workflows]]. Here any state may still move to any other; only the *category* invariants and *state-name* validity are enforced.
- Per-ticket-type / per-project workflow schemes (Jira's most-regretted complexity). One global workflow per repo.
- A GUI/DSL for defining states. Config-as-code in the versioned toml only.

## Sequencing

Step 2. Depends on [[closed-status-resolution]], which first splits `is_terminal()` from `completes_dependencies()` at the hard-coded `== Done` sites; this ticket turns those two predicates (plus dispatchable/open/parked) into registry lookups driven by config. Doing #1 first means this refactor generalizes an *existing, tested* seam rather than inventing the category model and the terminal-split simultaneously. Blocks [[state-transition-workflows]], which layers opt-in transition enforcement on the registry defined here.
