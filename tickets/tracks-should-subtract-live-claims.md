---
id: tracks-should-subtract-live-claims
title: tracks should account for live claims, not only ready-set conflicts
status: todo
priority: p2
dependencies: []
related: []
scopes: []
shared_scopes: []
paths: []
tags: [bug, orchestration, tracks]
---

## Observed (tiler repo, 2026-08-06, v0.13.0-era binary)

`tkt tracks` partitions the ready set into conflict-free batches, but "conflict-free" is computed only among the ready tickets themselves — scopes held by live `in-progress` claims are ignored. With `execute-the-doc-drift-sweep-the-audit-enumerated` claimed and in-progress holding `implementation/{compiler,ir,reference,build,metal-aot,frontend}`, batch 1 of `tkt tracks` still contained `admit-an-indirect-gather-family-for-tied-embedding-lookup`, whose scopes include three of those — and `tkt why` on that exact pair reports "cannot run in parallel, shared scope(s): implementation/compiler, implementation/ir, implementation/reference" (exit 6).

## Why it matters

The command's stated purpose is worker-sized dispatch fronts for an orchestrator ("an orchestrator with N workers gets worker-sized fronts"). An orchestrator that trusts a batch as dispatchable will claim work that collides with a live worker; the only defence is re-running `tkt why` pairwise against every live claim, which duplicates exactly the computation tracks exists to do. `ready` has the same blind spot but a weaker implied contract; `tracks` is where the batch is presented as safe.

## Proposal

Subtract live-claim-held scopes when composing batches by default: a ready ticket conflicting with an `in-progress` claim's scope set either lands in a later batch annotated with the blocking claim, or is listed under a "blocked by live claims" section rather than inside a batch. A `--ignore-claims` flag can restore today's behaviour for planning hypothetical schedules. Expired leases should probably still count as blocking until released, since the worktree may still exist — or at minimum be annotated distinctly.

## Reproduce

In a repo with claims: `tkt claim <a> --as x` where `<a>` shares a scope with ready ticket `<b>`; `tkt tracks` places `<b>` in batch 1; `tkt why <a> <b>` exits non-zero with the shared-scope conflict.
