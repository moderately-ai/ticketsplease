---
id: ux-empty-state-messages
title: Empty-state commands print 0 bytes
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, onboarding]
---
After init, list/ready/next/tracks/status all return 0 bytes exit 0 — ambiguous (worked? broken?), inconsistent with set/migrate which print no-op messages.
Fix: empty-state lines, e.g. list -> 'No tickets yet. Create one: tkt create --title ...'; ready/next/tracks -> 'No ready tickets (N total, M blocked).'
Found by: onboarding agent.
