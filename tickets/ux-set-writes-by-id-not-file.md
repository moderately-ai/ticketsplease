---
id: ux-set-writes-by-id-not-file
title: set/claim/release write by frontmatter id, not the file read
status: done
priority: p1
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
Commands resolve a ticket by filename but persist to <id>.md. When id != filename stem (after a rename or hand-edit), `set mismatch` reads mismatch.md but writes a NEW totally-different-id.md, orphaning the original and creating a duplicate id (lint flags it only after the fact).
Fix: write back to the same path that was read.
Found by: editing agent.
