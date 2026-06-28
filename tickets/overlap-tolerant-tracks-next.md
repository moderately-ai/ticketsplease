---
id: overlap-tolerant-tracks-next
title: "Overlap-tolerance dial: fill N workers least-cost-first (--max-overlap)"
status: todo
priority: p1
dependencies: [access-intent-scopes]
related: []
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

A throughput-versus-risk dial that fills N workers instead of idling them: prefer compatible/disjoint picks, then admit the cheapest overlaps to fill the rest, never forcing a single thread when an overlap is cheap to reconcile.

## Gap

`tracks` is all-or-nothing disjoint; `tracks --parallel N` only caps batch size. `next --allow-overlap` fills by raw score (not least-overlap) and the disjoint mode returns fewer than N — the forced single-threading.

## Work

- `--max-overlap <K>` on `next` and `tracks`: a per-pair cost budget. `0` (default) = compatible-only (strict, but shared×shared now counts as compatible); `K` = also tolerate conflicting pairs whose `conflict_cost` ≤ K; `any` = unbounded.
- Selection: take compatible high-score picks first, then greedily admit the lowest-marginal-cost overlaps to fill N (deterministic, tie-break by id).
- Surface cost: per pick `conflicts_with: [{ticket, scopes, cost}]` and a set-level `overlap_cost` total in JSON.
- `--allow-overlap` becomes an alias for `--max-overlap any` and adopts the least-overlap-first selection.

## Acceptance

Three tickets that all hold `core` exclusively, `next --parallel 3 --max-overlap 1`, returns all three with per-pair cost surfaced; `--max-overlap 0` returns one; cost totals are correct; no-flag behaviour is unchanged.
