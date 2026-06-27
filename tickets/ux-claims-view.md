---
id: ux-claims-view
title: No who-holds-what view / cannot steal a live lease
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, enhancement]
---
There's no first-class view of current claims + lease expiry beyond scanning status --all-branches, and stealing is only implicit on lease expiry (no way to force-take a live lease; --force exists only on release).
Fix: a `tkt claims` view (assignee + lease_expires_at + live/expired) and a supported way to force-steal a live claim.
Found by: orchestration agent.
