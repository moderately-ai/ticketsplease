---
id: ux-error-message-double-prefix
title: Doubled 'invalid input:' prefix in some errors
status: todo
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux]
---
Some errors double the prefix, e.g. `error: invalid input: <path>: invalid input: unknown status `wat``.
Fix: avoid re-wrapping an already-prefixed Error when annotating with the file path.
Found by: scripting agent.
