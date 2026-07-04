---
id: state-transition-workflows
title: "Opt-in state-transition enforcement (allowed transitions)"
status: done
priority: p2
dependencies: [config-custom-states]
related: [closed-status-resolution]
scopes: [core, cli, skill, docs]
paths: []
tags: [state-model, feature]
---
## Goal

Let a repo declare which state→state transitions are legal (`[workflow.transitions]`) and have `tkt` reject the rest — so a workflow can force review-before-done, forbid re-opening a shipped ticket, or require a QA gate. Strictly **opt-in and off by default**: the adjacency graph is a convenience constraint layered on top of the always-enforced category invariants from [[config-custom-states]].

## Gap

After #2, states and categories are configurable but `set --status` still writes any state over any other — there is no notion of an illegal move. Teams that genuinely need a gate (QA sign-off, approval, no-skipping-review) can express the *states* but not the *edges* between them.

## Design decisions (prior art)

This is the feature the survey warns about most loudly — enforced transitions are where Jira draws its heaviest criticism ("5 approval steps for a typo fix", "why can't I go from here to there?"). The dominant expert guidance is unambiguous: default to any-to-any, add restrictions only on evidence. So the design is deliberately minimal.

- **Off by default; `enforce_transitions = false`.** Category invariants (scope-locking, dispatch eligibility, dependency satisfaction) are *always* enforced because they are engine correctness — but they ride on categories, not edges. The state→state graph is, in the brief's words, "almost pure ceremony for a coordination engine", so it must be opt-in. ProjectFlow's rule: "add defined transitions only when you have EVIDENCE of problems."
- **A pure declarative adjacency map — no side-effects.** `[workflow.transitions]` maps `from = [to, …]`. That is the entire feature. **Explicitly do NOT build** transition validators, conditions, post-functions, transition screens, required-field-on-transition, or approval steps — that machinery, not the edge list, is Jira's actual sprawl engine and the source of the "unrecognizable workflow" complaints.
- **Escape hatches from day one.** A wildcard source `"*" = [...]` (e.g. close/cancel reachable from anywhere) and a CLI `--force` flag to bypass a single guarded transition. Every enforced system that lasts ships these.
- **Engine-driven transitions are exempt.** `claim` (→ the open state) and `release` (restore `claimed_from`) are engine mechanics, not user status edits, so they are never gated — otherwise a restrictive graph could wedge the claim/release cycle. Enforcement applies to user-initiated status changes (`set --status`, `close`, `reopen`).
- **Global workflow only.** One transition graph per repo; no per-ticket-type schemes.

## Work

**Core (`ticketsplease-core`)**
- Consume the `enforce_transitions: bool` and `transitions: BTreeMap<String, Vec<String>>` already carried on the `Workflow` struct from #2 (no new config migration needed).
- `Workflow::can_transition(from, to) -> bool`: `true` when enforcement is off, when `from == to` (a no-op re-set), when an explicit edge exists, or when a `"*"` wildcard permits `to`. Terminal-state outgoing edges allowed only if declared (so `reopen` from a terminal state must be an explicit edge or wildcard — which is the correct place to express "shipped work cannot be reopened").
- `lint.rs` / `doctor`: validate the graph against the registry — `unknown-transition-state` (an edge names a state not in `[workflow.states]`), and warnings for `unreachable-state` (no inbound edge from a non-terminal, non-initial state) and `dead-end-nonterminal` (a non-terminal state with no outbound edges → a ticket can wedge there).

**CLI (`ticketsplease-cli`)**
- Enforce in `set_single`/`set_bulk` (`commands.rs`), and in the `close`/`reopen` paths from #1, when `enforce_transitions`. On an illegal move, fail with a clear message naming the attempted `from → to` and the legal targets from `from`; use a distinct, documented exit code (a `Conflict`-class state error, à la the cycle-rejection exit).
- `--force` on `set`/`close`/`reopen` to bypass (records the override in the emitted status event so the audit trail shows it was forced).
- Bulk `set --where --status X` skips (with a per-ticket note) any matched ticket whose current state can't legally reach `X`, rather than aborting the whole batch — mirror the existing partial-result reporting.
- Optional: extend `tkt graph` to emit the workflow state machine as DOT (reuses the existing Graphviz export path), so the configured workflow is inspectable.

**Docs**: README + skill — document `[workflow.transitions]`, `enforce_transitions`, the `"*"` wildcard, `--force`, the claim/release exemption, and the "off by default, add on evidence" guidance so users don't cargo-cult a Jira-style graph.

## Acceptance

- With `enforce_transitions = false` (default) or no `[workflow.transitions]`, every transition is allowed — identical to #2's behaviour.
- With `enforce_transitions = true` and `review = ["done"]` but no `todo → done`, `set X --status done` on a `todo` ticket is rejected with a message listing the legal targets from `todo`; the same command with `--force` succeeds and the event records `forced: true`.
- `"*" = ["closed"]` lets any ticket be closed regardless of current state.
- `claim`/`release` continue to work even when the graph would not permit `todo → in-progress` directly (engine transitions are exempt).
- `doctor` flags an edge referencing an unknown state and warns on a non-terminal dead-end state.
- Bulk `set --where … --status review` advances the legal matches and reports the illegal ones as skipped, without aborting.

## Out of scope

- Transition side-effects of any kind (validators, conditions, post-functions, screens, required fields, approvals) — deliberately excluded; this is the Jira sprawl to avoid.
- Per-ticket-type or per-project workflow schemes.
- Auto-transitions / triggers (e.g. "move to done when the branch merges") — a separate automation concern, not this ticket.

## Sequencing

Step 3, the final piece of the initiative and the second half of the user's "config-driven states **and** state workflows" request. Depends on [[config-custom-states]] for the state registry, categories, and the `Workflow` config struct. Kept last and smallest on purpose: it is the highest-risk-of-over-engineering feature, so it ships only after the category model (which delivers most of the real value) is proven, and it stays a pure adjacency list with escape hatches.
