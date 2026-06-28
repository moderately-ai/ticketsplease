---
id: bulk-edit-manifest
title: Bulk set --where + TOML manifests
status: done
priority: p1
dependencies: [query-where-filters]
related: []
scopes: [core, cli]
paths: []
tags: [ergo, feature]
---
Tier-1 #3: authoring a batch was N round-trips. create --from already did JSON arrays; add TOML [[ticket]] manifests (extension/sniff detection) and bulk editing via set --where/--view (field edits applied to every match, one cycle check, dry-run, body/title rejected in bulk).
