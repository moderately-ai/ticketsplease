---
id: access-intent-scopes
title: "Access-intent scopes: shared (additive) vs exclusive locks"
status: done
priority: p1
dependencies: []
related: []
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

Let a ticket declare, per scope, whether it claims that area *exclusively* (a rewrite — today's behaviour) or *shared/additive* (append, extend, add a file). Two shared claims on the same scope are compatible and may run in parallel; the scheduler stops treating every shared scope as a hard conflict.

## Gap

Scopes are implicitly exclusive: `schedule::shares_scope` makes any two tickets that name a common scope conflict, so additive work on a hub scope (`core`) is forced to single-thread even though co-editing it is harmless.

## Work

- Frontmatter: add `shared_scopes: []`, a peer of `scopes` over the same `[scopes]` vocabulary. `scopes` = exclusive claims (default), `shared_scopes` = shared/additive claims. A scope must not appear in both.
- Core model: parse/render `shared_scopes` (mirror the `related` field), with `add_shared_scope`/`remove_shared_scope` mutators; `create`/`set` flags (`--shared-scope` / `--add-shared-scope` / `--remove-shared-scope`).
- Schedule: replace the binary `shares_scope`/`conflicts` with a compatibility test + `conflict_cost(a, b)`: for each scope both hold, `shared×shared` = compatible (cost 0); `shared×exclusive` and `exclusive×exclusive` = conflict (cost = scope weight, default 1). `tracks`/`next`/`why` consume the new compatibility.
- Lint: a scope in both `scopes` and `shared_scopes` (exit 3); both must be defined scopes.

## Acceptance

Two tickets sharing only a `shared_scopes` scope land in the same `tracks` batch and can be picked together by `next`; a shared×exclusive pair still conflicts; a ticket with no `shared_scopes` behaves exactly as today. `why` explains a shared-compatible pair as non-conflicting.

## Refs

Generalizes `schedule::shares_scope`/`conflicts`; the same shared/additive idea the guard already derives at diff time as `cause: direct|transitive`. Foundation for the rest of the parallel-control initiative.
