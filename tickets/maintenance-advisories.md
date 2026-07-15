---
id: maintenance-advisories
title: "EPIC: interactive maintenance advisories (update-check + drift/migrate nudge)"
status: todo
priority: p1
dependencies: []
related: [advisory-output-channel, maintenance-config-table, migrate-dry-run, update-check-advisory, drift-migrate-advisory, lint-summary-advisory, auto-migrate-apply, docs-maintenance-advisories]
scopes: []
shared_scopes: []
paths: []
tags: [epic, maint-advisory]
---
Two interactive maintenance advisories over one shared, strictly-gated channel: (1) an update-available notice, (2) a repo-drift / migrate nudge — plus a lint-findings summary and opt-in auto-migrate.

## Why

A sweep of 6 repos using ticketsplease found every one silently a release behind, and 2 boards holding 28 `paths-without-scopes` tickets that only surfaced on a manual `lint`. The tool has no way to tell a human "you are stale / your board drifted" without them thinking to run `doctor`/`lint`.

## The non-negotiable

Agent-first: every advisory is invisible to non-interactive use (TTY + human-format + non-CI + opt-out-able), stderr-only, never blocking, never a network stall. See [[advisory-output-channel]].

## Shape of the work

Foundations ([[advisory-output-channel]], [[maintenance-config-table]], [[migrate-dry-run]]) → features ([[update-check-advisory]], [[drift-migrate-advisory]], [[lint-summary-advisory]]) → opt-in [[auto-migrate-apply]] → [[docs-maintenance-advisories]]. Execute in dependency order; ships as its own minor release.
