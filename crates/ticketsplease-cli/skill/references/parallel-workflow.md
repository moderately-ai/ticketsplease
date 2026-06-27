# Multi-agent orchestration patterns

This is the workflow ticketsplease is built for: one orchestrator fanning out disjoint work to several workers, with a hard merge gate.

## The fan-out loop

```
while there is ready work:
    batches = `ticketsplease tracks --format json`.batches
    if batches is empty: break
    front = batches[0]                       # the immediately-dispatchable disjoint set
    for ticket in front:                     # one worker per ticket, in parallel
        `ticketsplease claim <ticket.id> --as <worker> --format json`   # exit 6 → already taken, skip
        dispatch the worker on branch tkt/<ticket.id> scoped to ticket.scopes
    wait for workers; merge each only if its guard passes (below)
    on success `ticketsplease set <id> --status done`; on abandon `ticketsplease release <id> --as <worker>`
```

Why only `batches[0]`? Every member of a single batch is scope-disjoint, so the whole front is safe to run at once. Later batches share a scope with a front member and should wait until the front merges and the graph is recomputed. (Dependency ordering is handled separately — only tickets whose dependencies are all done are ever offered, so batching gates on scope overlap alone.)

## Push or pull

The loop above is **push-based**: one orchestrator claims each ticket and hands it out. Because `claim` is atomic (a git-ref compare-and-swap), you can equally run **pull-based** — give every worker the same `tracks`/`ready` output and let each worker `claim` its own pick. Of any workers that race the same ticket, exactly one wins; the losers get exit 6 and simply move to the next ready ticket. No central coordinator, no double-assignment. Prefer this when you have a fleet of interchangeable workers rather than one orchestrator. Always claim *before* doing work, never after — the claim is what reserves the ticket; setting status by hand (`set --status in-progress`) is not race-safe and can let two workers grab one ticket.

A pull worker can collapse recommend-then-claim into a single race-safe call: `ticketsplease next --claim --as <worker> --format json` claims the best free pick, falling through to the next on a lost race. Use `ticketsplease claims --format json` to audit who holds what (assignee, lease, live/expired) and `claim --force` to take over a live lease deliberately.

## Branch naming

Name each branch so the guard can infer the ticket without `--ticket`: include the id, e.g. `tkt/<id>` or `<id>-short-description`. The guard picks the longest ticket id that appears in the branch name. When in doubt, pass `--ticket <id>` explicitly.

## The merge gate

A worker must pass the guard before its branch merges:

```
ticketsplease guard tkt/<id> --base main --format json
case exit:
  0 -> merge
  6 -> read the JSON:
         under_declared non-empty -> the branch edited files outside its declared area
             (declared-scope globs + paths). These are genuine escapes — narrow the diff back, or
             if the area is truly part of this ticket `ticketsplease set <id> --add-scope <scope>`
             (or add the file to the ticket's `paths`) and re-guard. The cargo reverse-dep
             expansion never lands here, so editing a foundational crate within your declared globs
             will not trip it.
         collisions non-empty -> another open ticket's declared area overlaps your affected set.
             Per collision check `cause`: `direct` = a real overlap, coordinate (merge the other
             first, or split the work); `transitive` = only the reverse-dep graph connects you,
             usually a false alarm for additive work. To auto-allow transitive-only collisions
             while still seeing them, gate with `guard --ignore-transitive` (exits 0 unless there's
             a direct overlap or under-declaration; `transitive_only: true` in the JSON marks the
             case). `--direct-only` instead drops the reverse-dep walk entirely. Don't let
             transitive noise train you to ignore a genuine exit 6.
  other -> a setup problem (4 = no ticket resolved, 3 = bad input); fix and retry.
```

The guard is the safety net that lets you dispatch aggressively: if two branches would collide, at least one fails the gate instead of producing a silent merge conflict. It is built for this flow: it reads the `[scopes]` contract from `--base` (not the checked-out branch, which may carry a stale/empty config), and it reads sibling tickets' in-flight status from their `tkt/*` branch tips — so a collision against a worker who is `in-progress` on its own branch fires even when your checkout still shows that sibling as `todo`. Override the config source with `--config-ref` and the branch namespace with `--prefix`.

## Observing workers mid-flight

Workers advance status on their own `tkt/<id>` branches, so an orchestrator on `main` can't see it via `list` (working-tree only) until merge. Three commands read across branches without a checkout:

- `ticketsplease events --watch --since <cursor>` — the **multiplexed** wake-on-event across *all* tickets at once: returns the moment any worker changes status, claims, releases, or comments. Events live in `.git` refs, so you see them **without waiting for a commit** (unlike `status`/`show --ref`, which read committed branch state). Loop it, advancing `--since` to the last id you saw, to consume the stream without missing a transition. Prefer this to spawning N single-ticket watchers.
- `ticketsplease comment add <id> --as <w> --body -` / `comment list <id> --ref tkt/<id>` — leave durable notes on a ticket (blocked-reasons, decisions, questions) and read a worker's notes from `main`. Each `comment add` also rings the event doorbell above.
- `ticketsplease status --all-branches` — every `tkt/*` branch's ticket status at its committed tip; a simple snapshot when you don't need the live stream.
- `ticketsplease reconcile` — diff the board against git: in-progress tickets with no branch (a dispatch that never started), live `tkt/*` branches whose ticket still reads todo/ready (work the board doesn't reflect), and orphan branches. The board and git drift independently; run this before each dispatch round (exit 3 on drift) so you can trust `tracks`/`ready` before fanning out.
- `ticketsplease watch <id> --until review --timeout <secs>` — block until one worker reaches a status (exit 0) or give up (exit 7). It auto-resolves the `tkt/<id>` branch.
- `ticketsplease show <id> --ref tkt/<id>` — read one ticket (and its comments) as committed on its branch.

**Dual-writer note:** claim *before* the worker branches. `claim` flips the ticket to `in-progress` in `main`, so when the worker branches off `main` the base and branch agree on status; the worker's later `set --status review` then merges cleanly. Writing status on `main` *after* the worker has already changed it on its branch is what produces a trivial status merge conflict.

## Keeping the graph honest

- Declare scopes **before** dispatching, not after — the guard compares actual diff against declared intent, so an honest declaration up front is what makes the collision math work.
- Prefer narrow scopes. A scope like `cli` that covers an entire crate forces everything touching that crate to serialize. Finer scopes (`cli/guard`, `cli/output`) unlock more parallelism — but only declare what a ticket truly needs.
- Run `ticketsplease lint` after bulk edits to catch dangling dependencies and cycles before they reach the scheduler.

## Cross-clone (multi-machine) propagation

The live signals are local to one clone by default. Events live under `refs/ticketsplease/events/*` and the claim locks under `refs/ticketsplease/claim/*`; comments are files under `<tickets_dir>/<id>.comments/`. None of these propagate via a plain `git push`/`pull`:

- **Events and claim refs** are custom refs outside `refs/heads/*`, so a default push ignores them. To share them across machines, push/fetch them explicitly, e.g. `git push <remote> 'refs/ticketsplease/*:refs/ticketsplease/*'` (and the matching fetch refspec in `.git/config`). Claim locks shared this way give cross-machine mutual exclusion; events shared this way give a cross-machine doorbell.
- **Comments** are working-tree files, so they propagate only once committed and merged like any other file — `git add <tickets_dir>/<id>.comments && git commit` on the worker's branch, then merge.

This is invisible in the common single-machine case (one shared working tree or sibling worktrees of one clone, where `.git` is shared) — but across machines, an orchestrator that never sets the refspec will see an empty `events`/`claims` view and conclude, wrongly, that nothing is happening. For multi-clone orchestration, configure the refspec once and commit the comments dir.

## One-shot, stateless

Every invocation is independent and offline — no daemon, and the only shared state is git-tracked: the ticket files plus the claim locks (refs under `refs/ticketsplease/claim/`). An agent calls `ticketsplease claim`/`next`, does the work, calls `ticketsplease guard`, sets status or releases, and moves on. This makes the tool safe to drive from a loop and trivial to retry — a retried or crashed run leaves no corrupt state, and an abandoned claim simply expires on its lease so the ticket returns to the pool.
