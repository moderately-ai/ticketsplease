---
id: ux-list-full-fields
title: list omits scopes/paths/deps/tags -> N+1 show to filter
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, scripting]
---
list returns only {id,title,status,priority}; ready/tracks add scopes; show/status add assignee/lease. To filter by scope or tag a consumer must N+1 `show` every ticket.
Fix: full-field list output (or a documented --fields/projection).
Found by: scripting agent. (Pairs with ux-list-filters-and-format.)
