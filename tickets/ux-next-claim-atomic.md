---
id: ux-next-claim-atomic
title: No atomic dispatch+claim (next->claim TOCTOU)
status: todo
priority: p2
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, enhancement]
---
next and claim are separate calls, so between `next` recommending X and your `claim X`, another worker can take X (your claim exit 6) and you must re-run next. next has no --as; claim has no --next.
Fix: add `next --claim --as <worker>` (or `claim --next`) to atomically claim the best ready+disjoint ticket, making the pull-based loop a single race-safe call.
Found by: orchestration agent.
