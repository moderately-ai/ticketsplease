---
id: mutation-plan-acceptance
title: Permanent acceptance suite + doctor stranded-txn
status: done
priority: p2
dependencies: [mutation-create-from-wire, mutation-set-bulk-wire, mutation-rename-delete-plans, mutation-migrate-wire, mutation-plan-docs]
related: []
scopes: [core, cli]
shared_scopes: []
paths: []
tags: [mutation-plan, testing]
---
## Initiative

Tag: `mutation-plan`. **When this ticket is done (and its deps), the initiative is complete** — every multi-ticket mutation path is on plan→validate→commit, docs are truthful, and the permanent suite gates regressions.

## Goal

Close gaps in the permanent acceptance suite, add doctor visibility for stranded txn dirs, and confirm no remaining false durability claims in skill.

## Gap

Wire tickets each add some tests; the full matrix (create F1–F4, set_bulk, rename recovery, migrate, doctor) may still have holes. Stranded `.ticketsplease/txn/` after crash needs operator visibility.

## Work

- Fill any missing suite B tests from the long-term plan not already owned by wire tickets (create/set/rename/migrate).
- `doctor`: warn on stranded/incomplete txn dirs under `.ticketsplease/txn/`.
- Final grep of skill/docs for multi-file atomic overclaims; fix stragglers.
- Confirm `tkt rollup --tag mutation-plan` shows all siblings done before closing this ticket.

## Acceptance

- Full permanent suite green (create all-or-nothing + dry-run fidelity + no silent rewrite; set_bulk commit; rename no dual-id after recover; migrate all-or-nothing; core txn recovery).
- Doctor reports stranded txn when present.
- All other `mutation-plan` tickets are `done`.
- Initiative complete: `rollup --tag mutation-plan` is fully done.

## Non-goals

- New product features beyond suite/doctor.
- Recipe saga tests.

## Refs

P6 of long-term plan; final gate for tag `mutation-plan`.
