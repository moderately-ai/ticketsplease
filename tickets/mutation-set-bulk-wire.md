---
id: mutation-set-bulk-wire
title: set --where/--view commits via MutationPlan
status: todo
priority: p2
dependencies: [mutation-txn-upsert, mutation-plan-validate]
related: [bulk-edit-manifest]
scopes: [cli]
shared_scopes: []
paths: []
tags: [mutation-plan, set]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

`set --where` / `--view` applies field mutations as one MutationPlan of upserts: whole-board cycle check, then a single `Store::commit` — no sequential `save` loop.

## Gap

`set_bulk` mutates in memory, cycle-checks, then loops `store.save`. Mid-loop failure leaves some matches updated and others not. Same partial-write class as batch create.

## Work

- Pre-plan: illegal status transitions still **skip** individual matches (not abort bulk) when not `--force` — filter before solidifying the plan.
- Build post-image upserts for changed tickets; `validate_plan` / ensure_acyclic as today for dep adds.
- `store.commit` once; best-effort status events **after** successful commit.
- Reject title/body in bulk unchanged.

## Acceptance

- Happy path bulk set matches current field-edit behavior and JSON/human summary.
- Cycle-forming bulk dep add still exit 5 with **no** partial saves from that invocation.
- No sequential save loop remains for bulk set.
- Existing bulk-set integration coverage still passes; add failure/all-or-nothing coverage if injectable.

## Non-goals

- Recipe multi-step atomicity.
- Single-ticket set rewrite (optional thin plan; not required).

## Refs

Depends on upsert txn + validate_plan. Related historical: `bulk-edit-manifest`.
