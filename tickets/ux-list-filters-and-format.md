---
id: ux-list-filters-and-format
title: list lacks filters, sort, count, and column alignment
status: done
priority: p3
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
list only filters by --status (no --scope/--priority/--tag), has no sort control, no count/header, and the id column isn't padded so titles don't line up.
Fix: add filters + sort, a count/header, and align columns.
Found by: onboarding + editing agents.
