---
id: dispatch-lanes
title: "Lanes planner: sequence conflicting work, don't drop it"
status: done
priority: p2
dependencies: [overlap-tolerant-tracks-next]
related: []
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

Plan concurrent *lanes* for N workers: each lane an ordered run of tickets safe to do back-to-back, lanes mutually low-conflict. When two tickets conflict, chain them on one lane (the later rebases on the earlier's merged result) instead of dropping one to a future batch.

## Gap

`tracks` emits batches that wait for a graph recompute; a conflicting ticket is deferred, not assigned. An orchestrator with N workers wants pre-planned lanes that keep every worker busy.

## Work

- `tkt lanes --parallel N [--max-overlap K]`: assign ready tickets to ≤N lanes minimizing cross-lane *concurrent* conflict; order within a lane by dependency then priority so the later ticket rebases on the earlier; emit per-lane ordered ids plus a recommended global merge order.
- JSON `{ lanes: [[{id,title,...}]], merge_order: [ids], overlap_cost }`.

## Acceptance

Two conflicting tickets land in the SAME lane (sequenced), not dropped; a set of compatible tickets fills N lanes; the merge order respects dependencies. Reuses the `conflict_cost` model from overlap-tolerant-tracks-next.
