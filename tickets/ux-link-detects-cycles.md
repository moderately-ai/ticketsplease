---
id: ux-link-detects-cycles
title: link accepts multi-node cycles (only blows up later)
status: todo
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
link catches self-dep (exit 3) and missing target (exit 4) but not multi-node cycles: a->b then b->a is accepted (exit 0, changed:true); the corrupt graph only errors later in ready/tracks/next (exit 5).
Fix: run cycle detection at link write time and reject (exit 5).
Found by: scripting agent.
