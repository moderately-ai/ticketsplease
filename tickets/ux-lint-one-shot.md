---
id: ux-lint-one-shot
title: lint short-circuits graph checks when files fail to parse
status: done
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux]
---
lint reports file-level errors (bad YAML/status, id<->filename) but SUPPRESSES graph checks (dangling deps, cycles) until all files parse. With parse errors present a dangling dep is not reported; after removing the bad files the same dangling dep IS reported. Consumers must re-run to convergence.
Fix: run graph checks even when some files fail to parse (report both classes in one pass).
Found by: scripting agent.
