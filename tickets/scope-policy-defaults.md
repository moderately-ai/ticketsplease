---
id: scope-policy-defaults
title: Per-scope policy defaults in config (set-and-forget intent)
status: todo
priority: p2
dependencies: [access-intent-scopes]
related: []
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

Let an operator set a scope's access policy once in config instead of annotating every ticket — the escape hatch for "`core` is always additive" or "the migration registry is always exclusive".

## Gap

Access intent is per-ticket only (`shared_scopes`), which is repetitive for an area with a consistent policy.

## Work

- `[scope_policy]` in `ticketsplease.toml`: per scope an optional default `mode` (`exclusive` | `shared`) and/or `weight` (conflict cost; `0` = free to share, higher = riskier).
- A ticket's explicit `scopes`/`shared_scopes` overrides the scope's default mode; the weight feeds `conflict_cost`.
- `doctor`/`lint` validate the table against the scope vocabulary.

## Acceptance

Marking `core` `shared` in config makes core-sharing tickets co-schedule with no per-ticket annotation; a per-ticket exclusive claim re-serializes one of them; a `weight` raises/lowers that scope's contribution to `--max-overlap`.
