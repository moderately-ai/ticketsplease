---
id: related-links
title: Non-blocking related links (related field)
status: done
priority: p1
dependencies: []
related: []
scopes: [core, cli]
paths: []
tags: [ergo, feature]
---
Second-round feedback (Tier-1 #1): dependencies[] was the only link type and is a hard scheduling blocker. Add a non-blocking related[] field recorded structurally but ignored by readiness/tracks/cycle-detection, so thematic cross-references are queryable/graphable without imposing order.
