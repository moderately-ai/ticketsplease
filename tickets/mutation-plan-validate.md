---
id: mutation-plan-validate
title: "core: validate_plan + move validate_write out of CLI"
status: todo
priority: p1
dependencies: [mutation-plan-types]
related: []
scopes: [core, cli]
shared_scopes: []
paths: []
tags: [mutation-plan, foundation]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

One core write-validation API that assesses a **post-image** board (snapshot ∪ planned mutations) for scopes, links, closed-without-complete deps, and cycles — so create/set/link share vocabulary and batch plans never validate a different graph than they commit.

## Gap

`validate_write` lives in CLI (`commands.rs`). Batch create builds a provisional peer graph (slugify) that can diverge from final auto-ids. `create_batch` gates `ensure_acyclic` under `--no-validate`; set/link always cycle-check on dep adds. Three layers (`Graph::build`, `ensure_acyclic`, `validate_write`) drift.

## Work

- New `crates/ticketsplease-core/src/validate.rs`:
  - Move CLI `validate_write` → `validate_ticket_links` (+ `WriteFields`)
  - `ValidationOptions { validate_refs: bool, validate_cycles: bool }`
  - `validate_plan(config, snapshot, plan, opts)` builds `known` from `materialize_board`, runs link checks, then `schedule::ensure_acyclic` when `validate_cycles`
- CLI create/set/link call sites use core (thin wrapper or direct call). No intentional product behavior change except code location — **except** the plan API must encode: batch write paths will always set `validate_cycles: true` (wired in create-from-wire).
- Unit tests: aggregate multiple ref problems into one Invalid; cycle → Error::Cycle.

## Non-goals

- Do not rewrite create_batch write loop here (that's `mutation-create-from-wire`).
- Do not merge `Graph::build` into write validation (scheduling stays strict separately).

## Acceptance

- All existing integration tests still pass.
- CLI no longer owns the real validate_write body.
- `validate_plan` + materialize_board unit tests cover multi-ticket ref + cycle assessment against planned finals.
- set/link still cycle-check on dep adds; lint codes vocabulary preserved where applicable.

## Refs

P1 of long-term plan; depends on `mutation-plan-types`.
