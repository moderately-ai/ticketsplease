---
id: ux-why-self-id-guard
title: why <id> <id> reports a ticket conflicting with itself
status: done
priority: p3
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux]
---
`why core core` -> conflict via shared scope(s): core. A ticket trivially shares all its scopes with itself.
Fix: short-circuit when a==b (no conflict, or reject as a usage error).
Found by: orchestration agent.
