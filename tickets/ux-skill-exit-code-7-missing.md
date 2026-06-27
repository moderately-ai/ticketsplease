---
id: ux-skill-exit-code-7-missing
title: Skill exit-code tables omit exit 7 (timeout)
status: done
priority: p2
dependencies: []
scopes: [skill]
paths: []
tags: [ux, docs]
---
SKILL.md and references/commands.md exit-code lists don't mention exit 7, though watch/events use it.
Fix: add `7 timeout` to the exit-code references in the skill docs.
Found by: onboarding agent.
