---
id: docs-skill-parallel-control
title: Document the access-intent model, the dial, lanes, and escape hatches
status: todo
priority: p2
dependencies: [access-intent-scopes, overlap-tolerant-tracks-next, dispatch-lanes, next-aware-of-inflight, parallel-width-query, scope-policy-defaults, parallel-escape-hatches, guard-honors-access-intent]
related: []
scopes: [docs, skill]
paths: []
tags: [parallel-control, feature, docs]
---
## Goal

Document the whole parallel-control surface so an operator (human or agent) can pick the level of control that suits them.

## Gap

The lock/access-intent model, the `--max-overlap` dial, `lanes`, `next --running`, the width query, per-scope policy, and the escape hatches are new and undocumented.

## Work

- README: the access-intent (shared vs exclusive) model, `[scope_policy]`, and the command surface.
- Skill: `references/commands.md` (every new command/flag + JSON shapes), `references/parallel-workflow.md` (the lock matrix, tolerant fan-out, lanes loop, `next --running`, width, escape hatches), `SKILL.md` (brief), and the `guide` text.

## Acceptance

Every new command/flag is documented with its JSON shape; `parallel-workflow.md` shows a tolerant fan-out and a lanes-based loop end to end; `tkt lint` stays clean.
