---
id: guard-honors-access-intent
title: Guard honours shared/additive scopes (align dispatch with merge gate)
status: done
priority: p2
dependencies: [access-intent-scopes]
related: [ux-guard-transitive-exit-gate]
scopes: [core, cli]
paths: []
tags: [parallel-control, feature]
---
## Goal

Close the loop: an overlap the scheduler tolerated because both tickets hold the scope as shared/additive should not be failed by the guard as if it were an exclusive clash.

## Gap

`guard` collides on declared-scope overlap regardless of access intent, so dispatch may tolerate an overlap the merge gate then blocks — re-introducing the friction at merge.

## Work

Teach `guard` the access mode: a collision on a scope that both the target and the open ticket declare `shared` is non-failing (or a distinct, non-gating cause), consistent with how `--ignore-transitive` treats reverse-dep-only collisions. An exclusive claim by either side still gates.

## Acceptance

Two tickets that both hold `core` as `shared`: `guard` passes (no gating collision) where two exclusive `core` tickets still fail (exit 6); the collision is still reported (with its mode) for visibility.
