---
id: mutation-migrate-wire
title: migrate remap/engine use multi-upsert commit
status: todo
priority: p3
dependencies: [mutation-txn-upsert]
related: [migrate-engine]
scopes: [core, cli]
shared_scopes: []
paths: []
tags: [mutation-plan, migrate]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

Status remap and frontmatter migrate engine apply multi-ticket writes through one MutationPlan commit so a single migrate invocation cannot leave a half-remapped board.

## Gap

`migrate` remap and `migrate.rs` engine loop sequential `save` / `write_atomic`. Mid-run failure leaves mixed states/schema versions.

## Work

- Build upsert plan for all tickets that need remap/backfill in that invocation.
- `store.commit` once (or one plan per intentional phase if product requires — prefer one plan per CLI invocation).
- Preserve migrate dry-run / reporting behavior.

## Acceptance

- Successful migrate matches prior end state.
- Injected mid-plan failure leaves board unchanged for that invocation (txn rollback).
- Existing migrate tests still pass.

## Non-goals

- Cross-invocation resume protocol beyond existing migrate design.
- Changing migration step definitions.

## Refs

P4 migrate half; related `migrate-engine`.
