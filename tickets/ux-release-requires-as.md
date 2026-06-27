---
id: ux-release-requires-as
title: Bare `release` (no --as) drops anyone's live claim
status: done
priority: p1
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
Help says only the holder may release without --force, but omitting --as releases another agent's live lease with no --force.
Repro: `claim x --as alice`; `release x --as bob` -> exit 6 (held by alice); `release x` (no --as, no --force) -> Released, exit 0, alice's lease silently gone.
Fix: require --as (reject mismatch without --force), or make a bare release require --force.
Found by: orchestration agent.
