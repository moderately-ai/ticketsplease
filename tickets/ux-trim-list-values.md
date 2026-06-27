---
id: ux-trim-list-values
title: Comma-separated list values aren't trimmed; empties kept
status: todo
priority: p2
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, bug]
---
`--scope 'a, b , c'` stores ["a"," b "," c"]; `--scope 'x,,y,'` keeps empty tokens; `--scope ''` stores [""]. ' b ' is a distinct scope no `--remove-scope b` will match; lint doesn't flag whitespace/empty scopes.
Fix: trim tokens and drop empties when splitting (create + set); reject empty values.
Found by: editing agent.
