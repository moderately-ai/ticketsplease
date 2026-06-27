---
id: ux-json-error-contract
title: "--format json emits no JSON on error paths"
status: todo
priority: p1
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, bug, scripting]
---
Despite 'JSON is the stable contract', every error path returns empty stdout + a plain-text `error: ...` on stderr; even `lint --format json` prints a structured body AND a trailing plaintext error. A JSON consumer can't get a machine-readable failure.
Fix: under --format json emit { schema_version, error: { code, message } } on stdout (and keep the exit code).
Found by: editing agent (scripting agent likely corroborates).
