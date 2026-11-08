---
id: doctor-dead-scope-globs
title: "doctor: warn when a scope glob matches no tracked file"
status: todo
priority: p2
dependencies: []
related: [ux-guard-unscoped-paths]
scopes: [cli]
shared_scopes: [core]
paths: []
tags: [ux, enhancement, onboarding]
---
## Gap

`doctor`'s `scope_globs` check (`crates/ticketsplease-cli/src/commands.rs:4479-4487`) only confirms the globs compile (`PathGlobMapper::new`). A glob with a typo, or one left behind after files moved, compiles fine and matches nothing. Its scope then never shows up in `guard`'s file-to-scope mapping, and nothing tells the user the scope map has a hole. `ux-guard-unscoped-paths` reports changed files outside every scope at guard time; this is the setup-time counterpart.

There is also no command that shows what a scope covers. Setting up a 9-scope config for a Go repo took an external script over `git ls-files` to confirm every glob matched something.

## Work

- In `doctor`, match each scope glob against tracked files (`git ls-files`, reusing the guard's matcher so semantics are identical). Report each glob that matches nothing as a non-gating warning naming the scope and the glob. Skip this outside a git repo.
- Expose per-scope coverage for inspection, for example `doctor --format json` listing matched-file counts per scope, or a small `scopes` subcommand. Overlapping scopes are legitimate (nested scopes such as `skill` inside `cli`), so report overlaps only as information, never as a warning.
- Scopes whose globs all match nothing before their first files exist (for example `tickets/**` before the first commit) should read as a warning, not a failure.

## Acceptance

- A config with a misspelled glob makes `doctor` print a warning naming that scope and glob, and still exit 0.
- A config whose globs all match passes with no new output beyond the existing line.
- JSON output carries the per-glob match counts.
