---
id: mutation-plan-types
title: "core: BoardSnapshot, IdAllocator, MutationPlan, plan_creates"
status: done
priority: p1
dependencies: []
related: [ux-batch-atomic-idempotent]
scopes: [core]
shared_scopes: []
paths: []
tags: [mutation-plan, foundation]
---
## Initiative

Tag: `mutation-plan`. There is no epic ticket — when every ticket with this tag is `done`, the transactional multi-mutation redesign is complete. This ticket holds the shared invariants for the rest of the initiative.

## Goal

Introduce the permanent pure planning API in `ticketsplease-core` so multi-ticket mutations allocate **final ids** before any validation or disk write — without changing CLI behavior yet.

## Gap

`create --from` validates against provisional ids (`slugify(title)`) while the write path may mint `slug-N` via `create_unique_idempotent`. There is no shared plan type; each CLI command invents its own validate-then-sequential-write loop. Core has only single-file create/save primitives.

## Invariants (whole initiative)

1. **Batch durability** — non-zero exit ⇒ that invocation left the ticket working tree unchanged; zero exit ⇒ full plan applied.
2. **Single planned-id graph** — one pure allocation freezes final ids; validate, dry-run, and commit use the same ids and edge strings.
3. **Edge fidelity** — `depends_on` / `related` tokens are literal final ids (never silently rewritten to a different id).
4. **Injectivity** — no two plan elements share a final create id; collide with disk only if bytes identical (`Unchanged`).
5. **Dry-run honesty** — dry-run results match a commit of the same plan (modulo concurrent external writers).
6. **Idempotent full re-run** — success then re-run ⇒ all `created: false`; atomic abort ⇒ same as first run.
7. **`--no-validate`** skips referential checks only (missing targets, undefined scopes, closed-without-complete). Never skips slugs, status registry, unknown keys, injectivity, multi-file atomicity, or **cycles**.
8. **Events** are best-effort after working-tree commit; ticket files are the durability boundary.

### Locked product defaults

- Auto-id / cross-ref: **literal final ids**; authors must set explicit `id` on every batch ticket referenced by another member's `depends_on` / `related`.
- Two auto-ids with the same title in one batch → distinct finals `base`, `base-2` in batch order.
- Delete does **not** auto-strip inbound edges (lint keeps reporting).
- Recipes are not cross-step sagas (permanent non-goal).

## Work

- New `crates/ticketsplease-core/src/plan.rs` (export from `lib.rs`):
  - `BoardSnapshot` (tickets + contents for Unchanged checks)
  - `IdAllocator` (pure occupancy: snapshot + earlier reservations)
  - `CreateSpec`, `PlannedCreate`, `PendingMutation` skeleton, `MutationPlan`
  - `plan_creates`, `materialize_board`
- Extract or golden-test the pure unique-id algorithm against `store::create_unique_idempotent` (same suffix rules: `base`, `base-2`, …; content-identical → Unchanged).
- `Store::snapshot_for_plan` (lenient load + read contents) if needed for planning inputs.

## Non-goals (this ticket)

- No CLI rewiring, no journal/commit, no behavior change to `create --from`.
- No silent edge rewrite; no alias DSL.

## Acceptance

- Core unit tests: IdAllocator matches create_unique_idempotent suffix + Unchanged short-circuit; injectivity; duplicate explicit ids in one plan fail or reserve correctly; materialize_board overlays planned creates onto snapshot.
- Existing CLI integration tests still green; `create --from` path unchanged.
- `plan` module is the permanent API later tickets extend (not a throwaway shim).

## Refs

Design: long-term plan under `.pi-subagents/chain-runs/e9eb1d23/context/05-long-term-plan.md` (P0). Historical partial fix: `ux-batch-atomic-idempotent` (validate-then-write only).
