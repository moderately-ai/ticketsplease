---
id: doctor-coverage-greenfield
title: "doctor: collapse scope_coverage warnings per scope and allow scopes declared ahead of their files"
status: todo
priority: p3
dependencies: []
related: [doctor-dead-scope-globs]
scopes: [cli]
shared_scopes: [core]
paths: []
tags: [ux, enhancement]
---
## Gap

`doctor-dead-scope-globs` warns per glob. In a new repo whose `[scopes]` are declared before the code exists (the normal way to set up a board for planned work), `doctor` prints one `!` line per glob: 20 lines for a 9-scope config on 2026-11-09 in `cliproxyapi-archiver`. The signal (a misspelled glob in an established repo) drowns in the noise (a greenfield repo), and the output stays noisy until every planned file exists.

## Work

- Human output: one line per scope listing its unmatched globs, e.g. `! scope_coverage: scope \`store\` matches no tracked file for 3 glob(s): src/store/**, src/store.rs, src/schema.rs`. Keep the per-glob detail in JSON.
- Allow a scope to opt out: `[scope_policy] "store" = { allow_empty = true }` (or a `[scopes_meta]` key if `scope_policy` is the wrong home) suppresses the warning for that scope while it has no files; `doctor` lists opted-out scopes in one informational line so the opt-out is not forgotten.
- Alternatively or additionally, suppress coverage warnings entirely when the repo has fewer than N tracked files, with the threshold in config; decide which fits the model better and document it.

## Acceptance

The 9-scope greenfield config above prints at most one line per scope, or none with opt-outs; a misspelled glob in an established repo still prints its warning naming the glob; JSON is unchanged except for an `allow_empty`/suppressed indicator.
