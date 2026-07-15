---
id: migrate-dry-run
title: "migrate --dry-run: preview without writing (fills a real gap)"
status: todo
priority: p1
dependencies: []
related: []
scopes: [core, cli]
shared_scopes: []
paths: []
tags: [maint-advisory, foundation]
---
`migrate` has no preview today (only `--remap/--repo/--format`). The drift advisory needs to compute "would migrate N ticket(s)" **without writing**, and users deserve a dry run of a mutating command. Note: `doctor` is already read-only — the thing that *applies* is `migrate`, so "dry-run mode" belongs here.

## Proposed shape

- core `migrate.rs`: `migrate(store, dry_run: bool)` — compute the render/diff as today, but skip `write_atomic` when `dry_run`. The report (`migrated` ids / `unchanged` count) is identical either way.
- cli `commands.rs` + `cli.rs`: a `--dry-run` flag that also skips the `--remap` saves and the skill relink, and reports would-be changes — human wording "would migrate N ticket(s)" / a JSON `dry_run: true` plus a `skill_relink_needed` bool.

## Done when

`migrate --dry-run` reports the same set a real run would but leaves the working tree byte-for-byte unchanged (integration test asserts no file writes + correct count); a core unit test covers the dry-run path; a real `migrate` still writes exactly as before.
