---
id: auto-migrate-apply
title: "opt-in auto-migrate: apply drift repair in interactive sessions only"
status: todo
priority: p2
dependencies: [maintenance-config-table, drift-migrate-advisory]
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [maint-advisory]
---
Some users want drift auto-repaired rather than merely flagged.

## Proposed shape

When `[maintenance] auto_migrate = true` **and** in advisory context, run a real `migrate` instead of only warning, then print what changed. This must **never** fire in JSON / CI / non-TTY / parallel contexts — `migrate` writes ticket files and the skill link, and would race concurrent workers or dirty a branch mid-dispatch.

## Open question (pending owner decision)

Config-gated only (recommended — a per-invocation flag is easy to forget), or also a per-command `--auto-doctor` / `--auto-migrate` override?

## Done when

With `auto_migrate = true`, an interactive human command repairs drift and reports the applied changes; with it unset, only the nudge shows; a JSON / CI / non-TTY run never auto-writes even with the flag set.
