---
id: query-where-filters
title: Boolean --where filter expressions
status: done
priority: p1
dependencies: []
related: [related-links]
scopes: [core, cli]
paths: []
tags: [ergo, feature]
---
Tier-2 #4 (first half): list filters were one value per axis and AND-only — no negation, no OR, no saved query. Add a core query engine (field:value with AND/OR/NOT and parens) wired into list --where; reused later by set --where, rollup, and saved views.
