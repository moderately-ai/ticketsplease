---
id: ux-release-restores-status
title: release always lands a ticket in `ready`, losing original status
status: done
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux]
---
Claiming a `todo` ticket then releasing it leaves it `ready`, silently changing the original status.
Fix: restore the pre-claim status (or document the intent).
Found by: editing agent.
