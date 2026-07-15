---
id: drift-migrate-advisory
title: "drift advisory: detect schema/skill drift offline, nudge to migrate"
status: done
priority: p1
dependencies: [advisory-output-channel, migrate-dry-run]
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [maint-advisory]
---
Right now you only learn a board has drifted (missing managed keys, a stale skill copy) if you happen to run `doctor`/`migrate`. Surface it.

## Proposed shape

Compose a cheap, **network-free** drift signal in advisory context:
- would-migrate count from `migrate(store, dry_run = true)` (delivered by [[migrate-dry-run]]);
- skill-link staleness from `skill::link_ok` (a real-copy or wrong link, not a symlink to canonical).

If either indicates drift, emit: `repo drifted: N ticket(s) need migration; skill link is stale — run `tkt migrate``.

## Done when

The nudge fires when a ticket is missing managed keys or the project skill is a stale real copy; it is silent on a clean repo; nothing is emitted or computed in JSON / CI / non-TTY; no network call.
