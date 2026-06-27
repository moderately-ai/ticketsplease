---
id: ux-batch-atomic-idempotent
title: create --from is non-atomic and not idempotent for auto-ids
status: todo
priority: p2
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, bug]
---
Batch create applies partially then errors (no rollback) when one element conflicts; re-running a batch whose element omits id duplicates it (batch-...-2.md), unlike content-addressed single create; and it reports unchanged items as 'created'.
Fix: validate-all-then-write (or roll back); dedup auto-ids by content/slug; report created vs unchanged separately.
Found by: editing agent.
