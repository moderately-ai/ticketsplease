---
id: ux-link-remove-dangling
title: Cannot remove a dependency once its target is deleted
status: done
priority: p2
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, bug]
---
`link dep --depends-on target --remove` validates target existence first, so after the target file is deleted the only way to clean the dangling reference is hand-editing — exactly the state lint complains about.
Fix: --remove should not require the target to exist.
Found by: editing agent.
