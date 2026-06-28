---
id: skill-best-practices
title: Audit bundled skill against skill-creator best practices
status: done
priority: p2
dependencies: []
related: []
scopes: [skill]
paths: []
tags: [ergo, docs]
---
Ran three Sonnet review agents against the official skill-creator guidance (frontmatter/validation, SKILL.md structure, references). quick_validate.py passes. Applied: description leads with user intent + correct voice; trimmed claim/guard steps that over-inlined reference detail; frontmatter schema -> pointer; reference pointers say WHEN to read; commands.md exit-code duplication removed + Contents ToC + cross-ref footer; parallel-workflow.md collapsed duplicated flag-teaching (kept the transitive-noise warning) + added the authoring/initiative loop. Also folded the v0.5.0 features (related, --where, view, rollup, graph/path, templates) into SKILL.md + references.
