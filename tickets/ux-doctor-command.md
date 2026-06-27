---
id: ux-doctor-command
title: Add a `tkt doctor` setup check
status: done
priority: p3
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
No single command validates setup: config present, git repo + a commit, scope globs compile, base ref exists.
Fix: add tkt doctor.
Found by: onboarding agent.
