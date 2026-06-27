---
id: ux-aggregate-commands-degrade
title: One malformed ticket black-holes every aggregate command
status: todo
priority: p1
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
A single unparseable/typo'd ticket makes list/ready/next/tracks/status all exit 3; only lint degrades gracefully. You can't even `list` to find your other tickets.
Fix: skip+warn on individual bad files in load_all-backed commands (or add --strict to opt back into hard-fail).
Found by: editing agent.
