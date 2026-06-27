---
id: ux-delete-archive-command
title: No delete / close / archive command
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
Removing a ticket means hand-rm of the file; done tickets clutter list forever with no archive/--hide-done. Lifecycle has no real end state.
Fix: add tkt delete (and/or archive), and list --hide-done / --all.
Found by: editing agent.
