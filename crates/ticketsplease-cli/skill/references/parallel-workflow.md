# Multi-agent orchestration patterns

This is the workflow ticketsplease is built for: one orchestrator fanning out disjoint work to several workers, with a hard merge gate.

## The fan-out loop

```
while there is ready work:
    batches = `ticketsplease tracks --format json`.batches
    if batches is empty: break
    front = batches[0]                       # the immediately-dispatchable disjoint set
    for ticket in front:                     # one worker per ticket, in parallel
        `ticketsplease set <ticket.id> --status in-progress`
        dispatch a worker on branch tkt/<ticket.id> scoped to ticket.scopes
    wait for workers; merge each only if its guard passes (below)
```

Why only `batches[0]`? Every member of a single batch is scope-disjoint, so the whole front is safe to run at once. Later batches conflict with the front (they share a scope or a dependency component) and should wait until the front merges and the graph is recomputed.

## Branch naming

Name each branch so the guard can infer the ticket without `--ticket`: include the id, e.g. `tkt/<id>` or `<id>-short-description`. The guard picks the longest ticket id that appears in the branch name. When in doubt, pass `--ticket <id>` explicitly.

## The merge gate

A worker must pass the guard before its branch merges:

```
ticketsplease guard tkt/<id> --base main --format json
case exit:
  0 -> merge
  6 -> read the JSON:
         under_declared non-empty -> the branch touched an area the ticket never claimed.
             Either narrow the diff back into scope, or, if the extra area is genuinely
             part of this ticket, `ticketsplease set <id> --add-scope <scope>` and re-guard.
         collisions non-empty -> another open ticket owns an affected scope. Coordinate:
             finish/merge the other ticket first, or split the work so the scopes don't overlap.
  other -> a setup problem (4 = no ticket resolved, 3 = bad input); fix and retry.
```

The guard is the safety net that lets you dispatch aggressively: if two branches would collide, at least one fails the gate instead of producing a silent merge conflict.

## Keeping the graph honest

- Declare scopes **before** dispatching, not after — the guard compares actual diff against declared intent, so an honest declaration up front is what makes the collision math work.
- Prefer narrow scopes. A scope like `cli` that covers an entire crate forces everything touching that crate to serialize. Finer scopes (`cli/guard`, `cli/output`) unlock more parallelism — but only declare what a ticket truly needs.
- Run `ticketsplease lint` after bulk edits to catch dangling dependencies and cycles before they reach the scheduler.

## One-shot, stateless

Every invocation is independent and offline — there is no daemon and no shared state beyond the git-tracked files. An agent calls `ticketsplease next`, does the work, calls `ticketsplease guard`, sets status, and moves on. This makes the tool safe to drive from a loop and trivial to retry.
