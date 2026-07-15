---
id: lint-a-ticket-that-declares-paths-but-no-scopes
title: lint should flag a ticket that declares paths but no scopes — the scheduler cannot see it
status: todo
priority: p2
dependencies: []
related: []
scopes: [core]
shared_scopes: []
paths: []
tags: [lint, parallel-control, footgun]
---
`paths` looks like it declares file intent to the scheduler. It does not: only `guard` reads it (as an under-declaration allowance). `tracks` / `why` / `lanes` / `next` gate purely on scope names via `schedule::conflicting_scopes`, which never touches `.paths`. So a ticket authored with `paths` and no `scopes` is **invisible to the conflict math**, and the tool will actively recommend co-scheduling two tickets that rewrite the same file.

## Repro

Two tickets declaring the identical path, no scopes:

```
$ grep -E '^(scopes|paths):' tickets/*.md
tickets/refactor-parser.md:scopes: []
tickets/refactor-parser.md:paths: [core/parser.rs]
tickets/rewrite-parser.md:scopes: []
tickets/rewrite-parser.md:paths: [core/parser.rs]

$ tkt lint
ok: no problems found

$ tkt why rewrite-parser refactor-parser
`rewrite-parser` and `refactor-parser` do not conflict — they can run in parallel.

$ tkt tracks
batch 1: refactor-parser, rewrite-parser
$ tkt tracks --width
2
```

`batch 1` is documented as "safe to run in parallel". Here it dispatches two workers onto one file, and nothing in the pipeline objects.

## Why this is worth a lint rather than just docs

It is not a hypothetical. A 48-ticket QuiltDB board had 32 tickets in exactly this shape — every one filed by an agent that had read `SKILL.md`, seen `paths` in the frontmatter schema, and reasonably concluded it was the file-level declaration. `tracks --width` reported **24 safe parallel workers** on a board where 29 tickets rewrote one crate. The board looked healthy: `lint` was clean, `reconcile` was clean, and the number was fiction. It was only caught because `tkt why` was run by hand on a pair known to collide.

The failure is silent, it is the exact failure the tool exists to prevent, and the authoring mistake is a natural one.

## Proposed shape

A new `lint` code — `paths-without-scopes` or similar — when `paths` is non-empty and **both** `scopes` and `shared_scopes` are empty. That predicate is unambiguous: the author declared file intent, so the ticket is not a scope-less decision/epic ticket (which is legitimate and must stay clean). Message should name the consequence, not the rule — something like "declares paths but no scopes: `tracks`/`why` cannot see this ticket and will co-schedule it with work that rewrites the same files".

Deliberately NOT proposed: linting every scope-less ticket. Decision tickets, epics and umbrellas legitimately have no scopes and no paths, and flagging them would train people to ignore the code.

## The alternative worth weighing (your call, not ours)

Resolve `paths` through the `[scopes]` glob map and feed the result into `conflicting_scopes` — i.e. make `paths` a first-class scheduling input, since `guard` already maps changed files to scopes exactly that way. That would make the field mean what it looks like it means, and would have silently fixed the board above with no authoring change.

We did not propose it as the primary fix because it changes scheduling semantics for every existing board: tickets that today co-schedule would begin to conflict, which is *correct* but is a behaviour change arriving in a patch release. If you want it, it probably wants a config opt-in and a note in the skill. The lint is the conservative half and is valuable on its own.

## Done when

`lint` reports the new code for a ticket with paths and no scopes; a scope-less, path-less ticket (decision/epic) stays clean; the code appears in the `lint` code list in `references/commands.md`; and `SKILL.md` states plainly that `paths` does not feed batching — today both docs describe `paths` only in guard terms, which is accurate but easy to read past.
