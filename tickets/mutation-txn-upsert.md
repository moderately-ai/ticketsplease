---
id: mutation-txn-upsert
title: "core: upsert mutations with backup/restore in commit"
status: todo
priority: p2
dependencies: [mutation-txn-create]
related: []
scopes: [core]
shared_scopes: []
paths: []
tags: [mutation-plan, durability]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

Extend `Store::commit` so multi-file **upserts** (and supporting delete/rename_dir ops as needed for later CLI) are journaled with backup/restore — one commit path for set_bulk, rename, migrate.

## Gap

Create-only txn cannot absorb set_bulk sequential saves or rename repoints. Without backups, mid-publish upsert failure cannot restore prior bytes.

## Work

- `PendingMutation::Upsert` with pre-overwrite copy to `staging/backup/<id>.md`.
- On rollback while publishing: restore backups; unlink `created_by_txn`; reverse recorded rename_dir if present.
- `plan_upserts` (or equivalent) building MutationPlan from full post-image ticket renders.
- Optionally add DeletePath / RenameDir op kinds if needed by the journal model (even if CLI not wired yet).
- Core tests: multi-upsert success; failure mid-upsert restores prior content; recovery after kill.

## Non-goals

- Wiring set_bulk / rename / migrate CLI (follow-on tickets).
- Changing claim/event semantics.

## Acceptance

- Core tests prove upsert plans are all-or-nothing under injected failure and crash recovery.
- Create-only plans from `mutation-txn-create` still work.
- No CLI behavior change required for this ticket alone.

## Refs

P4 durability half of long-term plan.
