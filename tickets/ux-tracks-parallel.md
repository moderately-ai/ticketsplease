---
id: ux-tracks-parallel
title: tracks has no --parallel N (cap to worker count)
status: todo
priority: p3
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
tracks emits the full partition; only next is worker-count-aware. An orchestrator with N workers can't ask tracks for a worker-capped front.
Fix: tracks --parallel N (limit/shape batches to N), or document next as the worker-capped path.
Found by: orchestration agent.
