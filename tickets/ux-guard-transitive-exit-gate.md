---
id: ux-guard-transitive-exit-gate
title: guard exit code can't distinguish transitive-only collisions
status: done
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, guard]
---
The `cause` field tags collisions direct/transitive (shipped, praised), but a transitive-only collision still exits 6 like a real direct overlap, so exit-code gating can't auto-distinguish without parsing JSON. Add `guard --ignore-transitive`: still computes and reports transitive collisions (keeps the cause visibility, unlike --direct-only which drops the reverse-dep walk), but exits 0 when every conflict is transitive (no under-declaration, no direct collision). Surface transitive_only in the JSON + a human hint.
Follow-up feedback from the orchestration operator (extends the reverse-dep-noise item).
