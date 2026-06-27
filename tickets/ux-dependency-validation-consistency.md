---
id: ux-dependency-validation-consistency
title: create --depends-on accepts dangling deps; link rejects them
status: done
priority: p2
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug]
---
`create --depends-on no-such-ticket` succeeds (exit 0); `link x --depends-on ghost` errors exit 4. Two inconsistent models for the same relationship.
Fix: pick one — validate eagerly in both, or permit + lint-check in both — and document it.
Found by: onboarding + editing agents.
