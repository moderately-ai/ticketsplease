---
id: ux-concept-guide-for-humans
title: Conceptual model not learnable from the CLI alone
status: done
priority: p2
dependencies: []
scopes: [cli, skill]
paths: []
tags: [ux, onboarding, docs]
---
All the 'why' (scopes, what a track is, scoring, guard's purpose) lives in the bundled skill; ready/tracks/why --help are bare one-liners and top-level help describes `skill` only as 'Manage the bundled Claude skill' — a human is never pointed at the getting-started guide.
Fix: a `tkt guide` command or top-level help footer pointing at SKILL.md; add after_help examples to the heavy commands (create/guard/claim).
Found by: onboarding agent.
