---
id: auto-migrate-apply
title: "opt-in auto-migrate: apply drift repair in interactive sessions only"
status: done
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

## Decision

**Both surfaces:** the `[maintenance] auto_migrate` config knob (durable) **and** a per-command `--auto-doctor` override flag. The flag wins for that one invocation. Both are still hard-gated to interactive human context — neither auto-writes in JSON/CI/non-TTY/parallel runs. (Naming: `--auto-doctor` is the requested name even though `migrate` is what applies; keep the familiar name, note internally that it runs migrate.)

## Done when

With `auto_migrate = true`, an interactive human command repairs drift and reports the applied changes; with it unset, only the nudge shows; a JSON / CI / non-TTY run never auto-writes even with the flag set.
