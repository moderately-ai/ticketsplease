---
id: ux-set-title-and-paths
title: set cannot edit title or paths (asymmetry with create)
status: todo
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
create accepts --title and --path, but set has no --title and no --add-path/--remove-path (only scope/tag list edits), so title and paths are CLI-uneditable after creation. Adding a dep is yet another command (link) vs create's --depends-on.
Fix: add set --title, set --add-path/--remove-path (and consider set --add-dependency) for symmetry.
Found by: editing agent.
