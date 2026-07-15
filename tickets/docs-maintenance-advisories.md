---
id: docs-maintenance-advisories
title: "docs: advisories, [maintenance] config, opt-outs, migrate --dry-run"
status: todo
priority: p2
dependencies: [update-check-advisory, drift-migrate-advisory, lint-summary-advisory, auto-migrate-apply]
related: []
scopes: [skill, docs]
shared_scopes: []
paths: []
tags: [maint-advisory]
---
Document the advisory subsystem once it lands.

## Scope

- `SKILL.md`: a short "maintenance advisories" note — what they are, that they are strictly interactive/human-only, and the `TICKETSPLEASE_NO_ADVISORIES` opt-out.
- `references/commands.md`: `migrate --dry-run`; the `[maintenance]` table and its defaults; update-check behaviour and cache location; the opt-out env vars.
- `README.md`: a brief mention under setup/maintenance.

## Done when

All three describe the advisories, the `[maintenance]` config, the opt-outs, and `migrate --dry-run`; the agent-first gating is stated plainly; no stale claims about `doctor` "applying" anything.
