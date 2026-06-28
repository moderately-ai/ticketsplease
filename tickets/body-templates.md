---
id: body-templates
title: Per-type body templates (create --template)
status: done
priority: p2
dependencies: []
related: []
scopes: [cli, skill]
paths: []
tags: [ergo, feature]
---
Tier-2 #5: the house body convention (Goal/Gap/Work/Acceptance/Refs) was enforced only by humans copying prior tickets. Embed example templates, seed them to .ticketsplease/templates/ on init, and add create --template <name> with {{title}}/{{id}} substitution (explicit --body wins; batch specs accept a template field too).
