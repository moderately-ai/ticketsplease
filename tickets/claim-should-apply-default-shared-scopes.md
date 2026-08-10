---
id: claim-should-apply-default-shared-scopes
title: claim should apply configured default shared scopes
status: done
priority: p2
dependencies: []
related: [set-should-edit-scopes-and-tags]
scopes: [core, cli, skill, docs]
shared_scopes: []
paths: []
tags: [feature, claim, guard, scopes]
---

## Observed (tiler repo, 2026-08-06, v0.13.0-era binary)

Every claimed ticket in the tiler workflow must declare `project/tickets` (the scope mapping `tickets/**`) in `shared_scopes`, because `tkt guard` does not treat a ticket's own file as implicitly shared and every worker branch edits its own ticket. But `tkt claim` leaves `shared_scopes` untouched, so a ticket claimed straight from `create` ships with `shared_scopes: []` — and the worker's first `tkt guard` run fails with `UNDER-DECLARED: project/tickets`. Two workers hit this in one day; each fixed it branch-side by hand, which also means the integration tree's copy disagrees with the branch copy until merge, feeding the stale-copy guard false-conflict problem the tiler AGENTS.md documents.

## Proposal

A repo-level config key, e.g. `[defaults] shared_scopes = ["project/tickets"]` in `ticketsplease.toml`, that `tkt create` writes into new tickets and/or `tkt claim` ensures on claim (claim-time is the stronger fix: it covers pre-existing tickets filed before the config key). Guard semantics stay unchanged — the default is declaration sugar, not an implicit grant, so the declaration remains visible in the frontmatter where audits read it. An explicit `shared_scopes` already present is left alone (merge, don't overwrite).

## Reproduce

In a repo whose workflow requires a shared tickets scope: `tkt create --title x --id x && tkt claim x --as a`, branch from the claim, edit `tickets/x.md`, commit, `tkt guard --base <claim-commit> <branch>` → UNDER-DECLARED on the tickets scope.
