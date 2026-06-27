---
id: ux-status-parse-ergonomics
title: "Status parse: no valid-values hint, case-sensitive"
status: todo
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux]
---
`unknown status doing` doesn't list valid values (priority's error does: 'expected p0..p3'); and status is case-sensitive (TODO/In-Progress -> exit 3) while titles are auto-lowercased.
Fix: list the valid statuses in the error and accept case-insensitively.
Found by: onboarding + editing agents.
