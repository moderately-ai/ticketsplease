---
id: ux-uniform-json-envelope
title: No uniform JSON result envelope across commands
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, scripting]
---
Each command names its payload differently (tickets/ready/batches/picks/diagnostics/events/comments/created), so a generic consumer can't extract 'the result' without per-command knowledge.
Fix: a documented stable per-command key, or a uniform { schema_version, data } wrapper.
Found by: scripting agent.
