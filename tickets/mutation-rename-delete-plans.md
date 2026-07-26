---
id: mutation-rename-delete-plans
title: rename + delete as first-class MutationPlans
status: done
priority: p2
dependencies: [mutation-txn-upsert]
related: []
scopes: [cli, core]
shared_scopes: []
paths: []
tags: [mutation-plan, rename, delete]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

`rename` and `delete` become single journaled plans so crash/failure never leaves dual-id boards, half-repointed edges, or ticket-without-comments residuals.

## Gap

Rename today: create new → N repoint saves → rename comments dir → delete old. Crash ⇒ dual files + inconsistent graph. Delete: remove ticket then comments dir separately; inbound deps/related left dangling (OK by policy) but multi-artifact residual is not.

## Work

- `plan_rename`: create new (excl) + upsert repoints (deps **and** related) + RenameDir comments + delete old — one commit with upsert backups.
- `plan_delete`: delete ticket file + comments dir in one commit.
- **Inbound edges:** do **not** auto-strip on delete (lint continues `missing-dep` / `missing-related`). Optional `--strip-inbound` is out of scope unless added as a follow-up.
- Recovery: after recover, board is either fully old or fully new for rename — never dual-id.

## Acceptance

- Rename success: same public JSON (old, new, repointed) as today; all inbound deps/related updated.
- Rename crash/recovery test (core or integration with fault injection): no dual-id residual after recover.
- Delete removes ticket + comments together; failure leaves both or neither for that invocation's artifacts.
- Existing rename/delete tests still pass.

## Non-goals

- Auto-strip inbound on delete.
- Recipe saga.

## Refs

P5 of long-term plan.
