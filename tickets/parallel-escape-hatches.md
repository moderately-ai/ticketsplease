---
id: parallel-escape-hatches
title: "Escape hatches: --assume-shared / --strict / --overlap-matrix"
status: done
priority: p2
dependencies: [overlap-tolerant-tracks-next]
related: [scope-policy-defaults]
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

Different operators have different tastes; no one should be stuck with our defaults. Give a global override at both ends of the spectrum plus a raw-data hatch for those who want to assign work themselves.

## Gap

The model assumes you annotate intent. Some operators want "just parallelize everything, I'll reconcile"; some want "ignore the annotations, be conservative"; some want the graph and will do their own bin-packing.

## Work

- `--assume-shared`: treat every scope claim as shared (collapse conflicts — pack N, reconcile at merge).
- `--strict`: treat every claim as exclusive (ignore `shared_scopes`/policy — today's conservative behaviour on demand).
- `--overlap-matrix` (JSON): the weighted compatibility graph for the ready set — every pair with its shared scopes and `conflict_cost` — so an external orchestrator assigns work itself.
- Apply on `next`/`tracks`/`lanes`.

## Acceptance

`--assume-shared` collapses all conflicts (one batch / N picks); `--strict` reproduces pre-feature behaviour ignoring `shared_scopes`; `--overlap-matrix` emits every ready pair with shared scopes + cost.
