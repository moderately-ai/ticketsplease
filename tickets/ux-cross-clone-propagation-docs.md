---
id: ux-cross-clone-propagation-docs
title: Document that events (refs) and comments don't push by default
status: todo
priority: p3
dependencies: []
scopes: [skill]
paths: []
tags: [ux, docs]
---
Events live under refs/ticketsplease/events/* and comments as files under tickets/<id>.comments/; neither propagates via a default git push/commit, so cross-clone (multi-machine) orchestration needs an explicit refspec and committing the comments dir. Fine for the shared-working-tree case, surprising across machines.
Fix: document this in the skill (parallel-workflow.md) and note the refspec/commit needed for multi-clone.
Found by: orchestration agent.
