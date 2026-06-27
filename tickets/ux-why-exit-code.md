---
id: ux-why-exit-code
title: why returns exit 0 even when conflict:true
status: done
priority: p3
dependencies: []
scopes: [cli]
paths: []
tags: [ux, scripting]
---
why exits 0 regardless of verdict, asymmetric with guard (exit 6), so `why a b && ...` can't gate without parsing JSON.
Fix: exit 6 on conflict (or add a --quiet/gating flag), 0 when parallel-safe.
Found by: orchestration agent.
