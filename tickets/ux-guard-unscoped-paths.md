---
id: ux-guard-unscoped-paths
title: guard is blind to files covered by no scope
status: todo
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
A changed file matching no scope glob (e.g. src/misc/util.rs) is invisible to guard, so two tickets can both edit unscoped paths and collide undetected.
Fix: warn when changed files match no scope ('N changed file(s) covered by no scope'), surfacing scope-map gaps.
Found by: orchestration agent.
