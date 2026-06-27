---
id: ux-human-timestamps
title: No human-readable timestamps anywhere
status: done
priority: p3
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
comment list/events/show show a nanosecond id; JSON `at` is epoch seconds; lease_expires_at is a quoted epoch string. A user can't tell when something happened without doing math.
Fix: render relative/ISO timestamps in human output.
Found by: editing agent.
