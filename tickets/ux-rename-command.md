---
id: ux-rename-command
title: No rename / change-id command
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
There's no way to rename a ticket id; doing it by hand (rename file or edit id) triggers the set-writes-by-id duplicate bug.
Fix: a tkt rename <old> <new> that moves the file and rewrites the id atomically.
Found by: editing agent.
