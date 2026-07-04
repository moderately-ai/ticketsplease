---
id: closed-status-resolution
title: "Closed status: terminal-but-not-done, with optional resolution"
status: todo
priority: p1
dependencies: []
related: [config-custom-states]
scopes: [core, cli, skill, docs]
paths: []
tags: [state-model, feature]
---
## Goal

Give work that ends **without being completed** — won't-do, duplicate, obsolete, superseded, cancelled — a first-class terminal status (`closed`) distinct from `done`, with an optional machine-readable reason and free-text note. Today the only terminal status is `done` ("completed successfully"), so abandoned work has nowhere honest to land: it either gets mislabelled `done` (poisoning `% done` rollups and, worse, silently satisfying its dependents) or lingers forever in `blocked`/`todo` as board noise.

## Gap

`Status` is a fixed 6-variant enum (`ticketsplease-core/src/ticket.rs:19`) with exactly one terminal value, `done`. "Terminal" is not even an abstraction — it is hard-coded as `== Status::Done` in ~9 sites that conflate two *different* questions: "is this ticket finished for scheduling purposes?" and "did this ticket deliver its output, so dependents can build on it?". Adding a non-completion terminal state forces those two questions apart. That seam is the reason this ticket is step 1 of the initiative (see Sequencing) — it is the minimum viable slice of the category model that [[config-custom-states]] later generalizes.

## Design decisions (prior art)

Surveyed Jira, Linear, GitHub, GitLab, Azure DevOps, YouTrack. Findings drive the decisions below.

- **One status field with a baked-in category — not a Jira-style orthogonal "resolution" field.** Jira's separate always-present Resolution field is the #1 documented source of confusion ("Done but unresolved" items that look complete but never close reports) and the *only* surveyed model with a reopen footgun. Modern trackers (Linear, GitLab's 2024 Status field, GitHub's two close-verbs, Azure, YouTrack) converged on the opposite: terminal-ness is a property *of the status value itself*. So `done` and `closed` are **both terminal** (excluded from scheduling), distinguished only by whether they satisfy dependents — `done` does, `closed` does not. [[config-custom-states]] later formalizes this as an engine-owned category set (`dispatchable | open | parked | terminal`) plus a per-terminal-state `satisfies_dependents` bit; this ticket introduces the two predicates that become config-derived there.
- **`done` satisfies dependents; `closed` does NOT.** This is the differentiator and the whole point. Every surveyed tracker gets the abandoned-blocker case wrong: Linear/GitHub silently unblock (a cancelled blocker counts exactly like a completed one, so a dependent proceeds on a foundation that was deliberately abandoned), while Jira never auto-clears the block (permanent deadlock). For a tool dispatching autonomous agents with no human watching each unblock, the silent-unblock failure is dangerous. We split the difference: a dependency is *satisfied* only by `done`; a dependent of a `closed` ticket is neither auto-readied nor silently deadlocked — it is surfaced as **orphaned: blocker closed → re-point, waive, or cascade-close.**
- **Reason = optional short *frozen* enum + optional one-line note, both in frontmatter.** Mirror GitHub ("few fixed reasons + free text"), not Jira's global customizable list (which drifts into per-team inconsistency). Enum: `duplicate | wontdo | obsolete | superseded | cancelled`. Optional, never mandatory-on-close (mandatory close reasons are the friction Jira teams complain about). Freeze the set early — GitHub adding `duplicate` in 2025 was a breaking change for API clients.
- **Reopen is atomic and self-cleaning.** Reopening clears `closed_reason`/`closed_note` in the same transition (Azure-style auto-clear), so live frontmatter always reflects only the current state. The prior reason survives in the git history / activity event — nothing is lost.

## Work

**Core (`ticketsplease-core`)**
- `ticket.rs`: add `Status::Closed` (renders `closed`, parses case-insensitively, extend the `FromStr` error enumeration). Add optional `closed_reason: Option<ClosedReason>` and `closed_note: Option<String>` fields with parse/render (surgical, round-trip-safe like the other managed keys). Add `ClosedReason` enum (`duplicate|wontdo|obsolete|superseded|cancelled`).
- **Introduce the category seam** — the crux refactor. Add `Status::is_terminal()` (`Done | Closed`, "finished for scheduling") and keep completion-satisfies-dependents as a *separate* predicate, `Status::completes_dependencies()` (`Done` only). Then split every current `== Status::Done` site by which question it is really asking:
  - *terminal / scheduling* → `is_terminal()`: `schedule.rs:52` (exclude from ready), `commands.rs:595,666` (clear lease on terminal), `commands.rs:1005` (`--hide-done`; rename/extend to hide terminal), `commands.rs:1510` (`watch` always-terminal short-circuit).
  - *dependency satisfaction* → `completes_dependencies()`: `claim.rs:91` (unmet-deps gate), `schedule.rs:75` (dep satisfied), `commands.rs:1198` (rollup blocked calc).
  - `rollup` `done`/`percent_done` (`commands.rs:1169`) stays keyed on `Done` specifically (a closed ticket is not a completed one); add a separate `closed` count to the rollup payload.
- **Orphan detection** (new): a dependent whose dependency is `closed` is neither ready nor silently blocked. Add a helper that classifies such dependents and thread it into `ready`/`next`/scheduler (exclude from the auto-ready pool) and into `rollup`/`doctor`/`why` (surface as `orphaned-by-closed-dep` with the blocker id + reason and the three remedies).
- `claim.rs:73` — `closed` is already non-claimable (only todo/ready/in-progress claim); just extend the not-claimable error message to name it.

**CLI (`ticketsplease-cli`)**
- Primary mechanism: extend `set` with `--reason <enum>` and `--note <text>` (write only alongside `--status closed`; reject with a clear error otherwise). Composes with bulk `set --where`.
- `reopen <id> [--status <todo|ready|...>]` convenience verb: moves a closed ticket back to an active status **and** clears `closed_reason`/`closed_note` atomically (raw `set` can't clear-on-transition cleanly). Default target `todo`.
- Optional sugar (may fold in or defer to a follow-up): a `close <id> [--reason] [--note]` verb wrapping `set --status closed`.
- Status-change event already fires (`commands.rs:604`); include `reason` in its `data` payload so the activity log carries the why.
- `--where` grammar: `status:closed` works for free via `FromStr`; add a `reason:<value>` field to the filter grammar (`query.rs`) so `list --where 'status:closed AND NOT reason:duplicate'` is expressible.

**Validation & docs**
- `lint.rs` / `doctor`: flag `closed_reason`/`closed_note` present on a non-`closed` ticket (stale after a hand-edit); surface the `orphaned-by-closed-dep` findings.
- README `status:` line (`README.md:65`) and `## Status` section (`:132`); skill lifecycle prose (`SKILL.md:85` "ready when … every dependency is done", `references/parallel-workflow.md`, `references/commands.md` `set`/`ready`/`rollup` entries) — document `closed`, the reason/note, reopen, and the orphaned-dependent behaviour.

## Acceptance

- `tkt set X --status closed --reason wontdo --note "superseded by new approach"` writes `status: closed`, `closed_reason: wontdo`, `closed_note: …`; the ticket drops out of `ready`/`next`/`tracks` and out of `rollup`'s `% done` (it is *not* counted done), and appears under a `closed` bucket.
- Given `B depends-on A`: with `A: done`, `B` is ready. With `A: closed`, `B` is **not** auto-ready and **not** reported as a normal in-progress block — `rollup`/`doctor`/`why B` report `B` orphaned by closed dependency `A` (reason wontdo) with the re-point/waive/cascade remedies. `claim B` refuses with a message naming the closed blocker, not a generic "unfinished dependency".
- `tkt reopen X` returns `X` to `todo` and removes both `closed_reason` and `closed_note`.
- `closed` is rejected by `claim` as non-claimable; setting `--reason` without `--status closed` errors.
- `list --where 'status:closed'` and `--where 'reason:duplicate'` filter correctly; a status/reason typo exits 3.
- Existing tickets (no `closed_*` keys) parse and migrate unchanged; `done` semantics are byte-for-byte unchanged.

## Out of scope

- Making the state set / categories configurable — that is [[config-custom-states]]. This ticket hard-codes `closed` and the terminal / `satisfies_dependents` split; #2 generalizes the enum into a config registry and folds `closed` in as a default state.
- Enforced transitions (can you go `done → closed`?) — that is [[state-transition-workflows]]. Here any status may still move to any other.
- Custom/user-defined reasons, mandatory reasons, and a cascade-close *prompt* (auto re-pointing dependents of a `superseded` ticket) — note as future niceties.

## Sequencing

Step 1 of the `state-model` initiative. Deliberately hard-codes `closed` and the terminal-category split rather than waiting for the full config machinery, because it ships the concrete user-requested value immediately and, more importantly, forces the cheap introduction of the `is_terminal()` / `completes_dependencies()` seam that de-risks [[config-custom-states]]. The only throwaway is a handful of match arms (the `Closed` variant becomes a default config state in #2); the valuable parts — the category seam, the reason/note plumbing, reopen, and the orphaned-dependent detection — all carry forward unchanged.
