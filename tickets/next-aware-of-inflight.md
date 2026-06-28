---
id: next-aware-of-inflight
title: "In-flight-aware next: best pick compatible with what's running"
status: done
priority: p2
dependencies: [overlap-tolerant-tracks-next]
related: []
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

The dispatch-loop primitive: when a worker frees up, recommend the best pick that is compatible (within the overlap budget) with the tickets still in flight — not just the best of the whole ready set.

## Gap

`next` scores against the entire ready set; it has no notion of what is currently running, so a freed worker can be handed a pick that clashes with a still-running sibling.

## Work

- `next --running <id,...>` (alias `--avoid`): drop or down-rank picks that conflict (per the compatibility model + `--max-overlap`) with the given in-flight ids.
- When omitted, optionally default the in-flight set to tickets currently `in-progress` with a live claim, so the common loop needs no args.

## Acceptance

With one ticket running that holds `core` exclusively, `next --running <it>` skips other exclusive-`core` tickets (unless within budget) and returns a compatible pick; an additive (`shared`) `core` ticket is still offered.
