---
id: ux-claim-done-exit-code
title: Claiming a done ticket returns exit 3, not 6
status: done
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux]
---
`ticket is done, not claimable` is a state conflict but is reported as generic invalid-input (exit 3), indistinguishable from a bad value. claim's other conflicts use exit 6.
Fix: return exit 6 for not-claimable-due-to-status.
Found by: scripting agent.
