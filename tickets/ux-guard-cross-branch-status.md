---
id: ux-guard-cross-branch-status
title: guard collision check is cross-branch-blind
status: done
priority: p1
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug, scripting]
---
guard loads the ticket set via store.load_all() from the CURRENT checkout, and a collision only fires against siblings whose status is in-progress/review IN THAT CHECKOUT (is_open). In the branch-per-ticket flow each ticket's open status lives on its own branch, so a guarded branch sees siblings as todo -> NO collision, even though why/tracks/status --all-branches all know they overlap.
Repro: api-endpoints and api-ratelimit both in review on their tkt/* branches, both touch src/api/**; `guard tkt/api-endpoints --base main` from the branch checkout -> collisions:[], exit 0; only after forcing api-ratelimit in-progress into the local checkout does the COLLISION fire.
Fix: make guard consult tkt/* tip statuses (reuse the status --all-branches scan), or accept the in-flight ticket set, so collision detection works in the documented workflow. (Ties to the deferred actual-vs-actual work; pairs with ux-guard-config-source.)
Found by: orchestration agent.
