---
id: mutation-create-from-wire
title: create --from uses plan+txn (fix F1–F4; true all-or-nothing)
status: todo
priority: p1
dependencies: [mutation-plan-types, mutation-plan-validate, mutation-txn-create]
related: [ux-batch-atomic-idempotent]
scopes: [cli, core]
shared_scopes: []
paths: []
tags: [mutation-plan, create]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

Rewrite `create --from` (and preferably single `create`) so parse → `plan_creates` → `validate_plan` → dry-run emit **or** `Store::commit` — fixing partial writes, provisional/final id skew, BTreeMap collapse, and dry-run lies.

## Gap (reproduced)

| ID | Failure |
|----|---------|
| F1 | Mid-batch write leaves partial files (e.g. duplicate explicit ids fail after earlier creates land). |
| F2 | Validate uses slugify; write may mint slug-N; depends_on/related stay user strings → wrong graph. |
| F3 | BTreeMap known collapses colliding provisional ids. |
| F4 | Dry-run reports provisional ids / path.exists, not final plan. |

Historical `ux-batch-atomic-idempotent` only delivered validate-then-write + content-idempotent auto-ids.

## Work

- Rewrite `create_batch` in `commands.rs`:
  1. parse_manifest → CreateSpecs (status/slug always)
  2. plan_creates (final ids frozen; {{id}} bound to final)
  3. validate_plan: `validate_refs: !no_validate`, **`validate_cycles: true` always**
  4. dry_run → emit plan results; no disk
  5. store.commit → emit_create_results
- Prefer single `create` on the same plan path (one codepath).
- Edge tokens remain **author strings** (no silent rewrite). Hint in error.message when a planned peer's base slug equals a missing target but final_id differs.
- Keep public JSON shape: `{schema_version:1, results:[{id,created,path}], dry_run}`.
- Exit codes unchanged: 3 invalid, 5 cycle, 0 success/Unchanged re-run.

## Acceptance

- **F1:** duplicate explicit ids different bodies → exit 3, **zero** new files from that invocation.
- **F2/F3:** two same-title auto-ids get distinct finals; edges to missing after suffix do **not** silent-rewrite; exit 3 + nothing written when deps wrong.
- **F4:** dry-run `results[].id` / `created` match subsequent real create of same manifest (same snapshot).
- Cycle in batch → exit 5, nothing written; `--no-validate` still rejects cycles, bad slugs, unknown keys.
- Intra-batch explicit-id deps still succeed; existing batch tests still pass.
- Full re-run after success → all `created: false`.

## Non-goals

- set_bulk / rename / migrate wiring (later tickets).
- Skill/README rewrite (see `mutation-plan-docs`).
- schema_version bump.

## Refs

P3 of long-term plan. Closes the create surface of the initiative's user-visible correctness.
