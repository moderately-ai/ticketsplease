---
id: ux-claim-dependency-aware
title: claim succeeds on a ticket whose dependencies aren't done
status: todo
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
claim gates on status (done/blocked/review -> not claimable) but never checks dependencies, so `claim web-ui` succeeds (exit 0, -> in-progress) even though web-ui depends on an unfinished ticket and ready/next correctly exclude it. Inconsistent with the dispatch side.
Fix: refuse (or warn) when a claimed ticket's deps aren't all done.
Found by: orchestration agent.
