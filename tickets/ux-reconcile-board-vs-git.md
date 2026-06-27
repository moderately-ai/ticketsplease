---
id: ux-reconcile-board-vs-git
title: board status drifts from branch/worktree reality with no reconcile
status: done
priority: p1
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, scripting, orchestration]
---
Ticket status (markdown) has no link to whether the tkt/<id> branch or its worktree actually exists, so the board goes stale both ways: in-progress with no branch (stale-busy / never-started dispatch), and a live tkt/* branch+worktree while the board says todo (stale-idle / untracked work). The operator hand-reconciled with git show-ref + ls each time. Add `tkt reconcile [--prefix tkt/]` cross-referencing each ticket's status against tkt/* branches and git worktree list, flagging (a) in-progress tickets with no branch, (b) todo/ready tickets with a live branch, and (c) orphan tkt/* branches with no ticket. Highest-value addition for the multi-agent loop; same root cause as the cross-branch-state item.
Follow-up feedback from the orchestration operator.
