---
id: ux-lease-type-consistency
title: "lease_expires_at: quoted string in frontmatter vs int in JSON"
status: todo
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux]
---
Frontmatter stores lease_expires_at as a quoted string ("1782576649") while JSON emits an integer (1782576649).
Fix: write it unquoted in frontmatter to match the JSON type.
Found by: orchestration + editing agents.
