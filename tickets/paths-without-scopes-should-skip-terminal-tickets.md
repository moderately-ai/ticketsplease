---
id: paths-without-scopes-should-skip-terminal-tickets
title: paths-without-scopes fires on done/closed tickets, where its own claim is false
status: todo
priority: p2
dependencies: []
related: [lint-a-ticket-that-declares-paths-but-no-scopes]
scopes: [core]
shared_scopes: []
paths: []
tags: [lint, parallel-control, bug]
---
Follow-up to `lint-a-ticket-that-declares-paths-but-no-scopes` (thanks for the fast turnaround — both fixes landed exactly right, and `repointed references` is a better word than what we suggested). The new lint has no status guard, so it fires on terminal tickets, where the message it prints is **factually untrue**.

## The claim vs the behaviour

```
$ grep -E '^(status|scopes|paths):' tickets/shipped-last-year.md
status: done
scopes: []
paths: [core/parser.rs]

$ tkt lint
[paths-without-scopes]: declares paths but no scopes: `tracks`/`why` cannot see
this ticket and will co-schedule it with work that rewrites the same files …

$ tkt ready          -> 0 tickets
$ tkt tracks         -> (no ready tickets)
$ tkt tracks --width -> 0
```

`tracks` partitions **the ready set**, and `ready` is dispatchable-status only. A done ticket is never offered, so it cannot be co-scheduled with anything. The diagnostic describes a hazard that cannot occur.

## Why this is worth fixing rather than living with

It reproduces the exact failure the original ticket argued against. That ticket said:

> Deliberately NOT proposed: linting every scope-less ticket. Decision tickets, epics and umbrellas legitimately have no scopes and no paths, and flagging them would train people to ignore the code.

The same argument lands on terminal tickets, and we walked into it immediately. On upgrading to 0.10.0 our board went from green to **23 findings — 20 done, 3 closed, zero open**. Every dispatchable ticket was already correctly scoped; every finding was unactionable. `lint` exits 3, and the repo rule "lint and reconcile both exit 0 before a dispatch round" now blocks on noise.

The available responses are all bad, which is the tell:
- backfill scopes onto 23 terminal tickets to satisfy a lint that should not fire — churn on historical records, and the pattern the project treats as changing the test to make it pass;
- stop gating dispatch on `lint` — throws away the real codes;
- learn to read past a red `lint` — the exact outcome the original ticket set out to avoid.

This will hit any board that adopts the lint with history, which is every board that has been running long enough to need it.

## Fix

Skip the check for tickets in a `terminal`-category state. Everything needed is already in scope at the call site: `registry` is used ~10 lines below for `unknown-state` (`lint.rs:192`), and `states.rs:115` has `is_terminal()`. Roughly:

```rust
if !ticket.paths.is_empty()
    && ticket.scopes.is_empty()
    && ticket.shared_scopes.is_empty()
    && !registry.get(&ticket.status).is_some_and(StateDef::is_terminal)
{ … }
```

An unknown status should keep tripping the lint (it is already flagged `unknown-state`, and treating it as terminal would hide a second real problem).

## The one judgement call

`parked` (e.g. `blocked`) is not dispatchable *today* but is expected to become so. We would keep linting it — an epic parked on an unbuilt dependency still wants its scopes right before it is unparked, and unlike a done ticket its hazard is real, just deferred. So the predicate is `terminal`, not `!dispatchable`. Flagging your call in case you read the categories differently.

## Done when

A done/closed ticket with paths and no scopes lints clean; a todo/ready/blocked one still trips; a test covers the terminal carve-out; and the code list in `references/commands.md` notes the exemption.
