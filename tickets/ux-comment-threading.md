---
id: ux-comment-threading
title: "--reply-to is stored but never shown or validated"
status: done
priority: p3
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux]
---
comment --reply-to is recorded in JSON but comment list/show render flat (no nesting/indent), and an unknown --reply-to id is accepted (orphan reply), unlike link's strict validation.
Fix: render threads (indent / 'in reply to'); validate --reply-to against existing comment ids.
Found by: editing agent.
