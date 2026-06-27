---
id: ux-clear-claim-on-done
title: set --status done leaves a stale claim (assignee/lease)
status: todo
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
After completing a claimed ticket, show reports status:done but still assignee:'agent-1' and a live lease_expires_at. Anything computing 'who's working on what' from assignee sees a done ticket as owned.
Fix: auto-clear assignee + lease on a terminal status (done), or document that completion doesn't release.
Found by: scripting agent.
