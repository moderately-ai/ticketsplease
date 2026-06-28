---
id: parallel-width-query
title: "Parallel-width query: how wide can I safely go right now"
status: todo
priority: p3
dependencies: [access-intent-scopes]
related: [overlap-tolerant-tracks-next]
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

Tell the orchestrator how many workers it can safely spin up before it commits — the natural parallelism of the current ready set.

## Gap

There is no way to ask "what is my safe width right now"; you infer it by eyeballing `tracks`.

## Work

Compute the safe width = the size of the largest mutually-compatible set of ready tickets (within the active `--max-overlap` budget). Expose it as a `width` field on `tracks`/`next` JSON (and a one-shot `tracks --width`), and include it in `rollup`.

## Acceptance

Width equals the largest compatible ready set; it rises when scopes are marked `shared_scopes` or when `--max-overlap` is raised; it is 1 when every ready ticket exclusively shares one scope.
