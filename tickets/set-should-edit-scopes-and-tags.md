---
id: set-should-edit-scopes-and-tags
title: set should edit scopes, tags, and links, not only status and comments
status: todo
priority: p3
dependencies: []
related: []
scopes: []
shared_scopes: []
paths: []
tags: [feature, cli, set]
---

## Observed (tiler repo, 2026-08-06, v0.13.0-era binary)

`tkt set <id> --status done` works; `tkt set <id> --scopes implementation/compiler` fails with `error: unexpected argument '--scopes' found` (the tip suggests `--comments`). Filing a new ticket therefore requires `tkt create` followed by hand-editing the frontmatter to set `scopes`, `shared_scopes`, `tags`, and `related` — the fields that matter most for scheduling correctness, since scope declarations are what `guard`, `why`, and `tracks` compute from.

## Why it matters

Hand-edited frontmatter bypasses whatever validation `set` could do at write time (unknown scope names, malformed lists) and only surfaces at the next `tkt lint`. For an orchestrator filing several tickets per cycle, the create-then-hand-edit dance is the highest-frequency manual YAML editing in the whole workflow. (`tkt link` covers `dependencies`; nothing covers `scopes`/`shared_scopes`/`tags`/`related`.)

## Proposal

Either extend `tkt set` with `--scopes`, `--shared-scopes`, `--tags`, `--related` (comma-separated, with add/remove forms like `--add-scope`/`--remove-scope`), validating scope names against `ticketsplease.toml` at write time — or accept the same flags on `tkt create` so a ticket can be born fully declared in one command. Both would be better than either alone; `create` flags remove the window in which a ticket exists undeclared.
