---
id: ux-sanitize-ticket-id
title: Sanitize/validate --id (path traversal + crash)
status: done
priority: p0
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug, security]
---
--id is written to the filesystem verbatim, unvalidated.
Repro: `create --id ../../pwned` writes /tmp/pwned.md OUTSIDE the repo; `--id 'Has Space/UPPER'` -> raw OS error, exit 1 (not in the contract); `--id UPPER`/`--id 'with space'` accepted, lint says ok. Same code path is used by `create --from` id, so LLM-authored batch JSON is an injection vector.
Fix: slugify or reject ids (lowercase [a-z0-9-], no '/'/'..'/whitespace) before any FS write, error at exit 3; add a lint rule for non-slug ids.
Found by: onboarding + editing agents.
