---
id: ux-lint-cycle-exit-code
title: lint exits 3 on a dependency cycle (should be 5)
status: done
priority: p1
dependencies: []
scopes: [cli]
paths: []
tags: [ux, bug]
---
lint returns exit 3 on a cycle; the documented contract and sibling `ready` use 5.
Repro: cycle a->b->a: `lint` prints the cycle then `error: 1 problem(s) found` exit 3; `ready` exits 5.
Fix: map a cycle finding in `lint` to Error::Cycle (exit 5).
Found by: onboarding agent.
