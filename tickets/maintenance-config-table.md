---
id: maintenance-config-table
title: "config: [maintenance] table (update_check, auto_migrate, check_interval_hours)"
status: todo
priority: p1
dependencies: []
related: []
scopes: [core]
shared_scopes: []
paths: []
tags: [maint-advisory, foundation]
---
Config knobs the advisories read. Foundation for gating the update check and for opt-in auto-migrate.

## Proposed shape

A `[maintenance]` table in `config.rs`, parsed with serde defaults so an absent table means built-in defaults:

- `update_check: bool = true`
- `auto_migrate: bool = false`
- `check_interval_hours: u64 = 24`

## Done when

The table round-trips; defaults apply when the table (or a key) is absent; a config unit test covers parse + default; `lint` still passes on a config that omits the table (no false positive).
