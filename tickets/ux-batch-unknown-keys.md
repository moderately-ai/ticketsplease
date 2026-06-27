---
id: ux-batch-unknown-keys
title: Batch JSON silently drops unknown keys
status: done
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
`create --from` ignores unknown fields, so a typo like "dependson" (for depends_on) is silently dropped and the dependency is lost.
Fix: serde deny_unknown_fields on the batch spec, or warn on unknown keys.
Found by: editing agent.
