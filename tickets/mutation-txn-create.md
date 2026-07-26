---
id: mutation-txn-create
title: "core: journaled Store::commit for creates + recovery"
status: done
priority: p1
dependencies: [mutation-plan-types]
related: []
scopes: [core]
shared_scopes: []
paths: []
tags: [mutation-plan, foundation, durability]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

Make multi-create durability a **store primitive**: stage + journal + publish/rollback/recover so a plan of creates is all-or-nothing under failure and crash, independent of CLI.

## Gap

`create_exact` / `create_unique_idempotent` / `write_atomic` are single-file only. Batch create loops them with no rollback. Mid-batch failure leaves partial tickets. Comments claim “all-or-nothing” for validate-phase only.

## Work

- Journal under `.ticketsplease/txn/<txn_id>/` (not inside `tickets_dir`): `journal.json`, `staging/`.
- Phases: `staging` → `publishing` → `committed`.
- `Store::commit(&MutationPlan)` for **`PendingMutation::Create` only** in this ticket:
  - Created: stage → O_EXCL publish → track `created_by_txn`
  - Unchanged: skip O_EXCL; race-check content still matches
  - On error while publishing: unlink `created_by_txn` (reverse), discard staging
- `recover_pending_txn` on `Store::open` and at start of `commit`:
  - staging → drop txn dir
  - publishing → rollback creates + drop
  - committed → cleanup txn dir
  - corrupt journal → Internal, do not guess
- fsync stage files + journal; mirror create_exclusive publish semantics.
- Commit does **not** validate, emit events, or hold claim refs.

## Non-goals

- Upsert/delete/rename_dir ops (later tickets).
- CLI create_batch rewiring.
- Ticket WT + git event dual atomicity.

## Acceptance

- Core tempdir tests: successful multi-create; failure after N publishes rolls back all txn creates; recover after simulated kill in publishing; O_EXCL clash with different content aborts full txn; Unchanged entries do not delete pre-existing files.
- CLI behavior unchanged until `mutation-create-from-wire`.

## Refs

P2 of long-term plan; depends on `mutation-plan-types`. Positive multi-op pattern to mirror: `prune_events_before` update-ref --stdin (git side only).
