---
id: ux-naming-id-vs-ticket
title: Inconsistent JSON field naming (id/ticket, depends_on/dependencies, created)
status: todo
priority: p2
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, scripting]
---
The ticket id is `id` in show/list/set/ready/status but `ticket` in comment/events/next.conflicts_with[]/guard.collisions[] (and `id` is overloaded for comment/event ids). Input uses `depends_on` (create --from, link --depends-on) while output/storage uses `dependencies`. And `created` is a bool on single create but an array on batch create (same key, different type -> forces type-sniffing).
Fix: pick `id` everywhere for the ticket; align input depends_on with output dependencies; make single/batch create share one result shape.
Found by: scripting agent (created-type also seen by editing agent).
