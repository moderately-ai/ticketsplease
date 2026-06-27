---
id: ux-lint-diagnostic-codes
title: lint/why diagnostics are freeform; exit 3 is a grab-bag
status: done
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux, scripting]
---
lint/why messages are freeform strings a consumer must regex; and exit 3 covers bad value, dup id, bad --from JSON, self-dep, missing config/repo, dangling dep, not-a-git-repo, and not-claimable. Can't distinguish by code.
Fix: add a typed code/kind to diagnostics (cycle|missing-dep|bad-status|yaml|id-mismatch) and carry an error `code` in the JSON error envelope (see ux-json-error-contract).
Found by: scripting agent.
