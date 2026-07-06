# Multi-agent orchestration patterns

This is the workflow ticketsplease is built for: one orchestrator fanning out disjoint work to several workers, with a hard merge gate.

## Authoring an initiative (before you dispatch)

Turning a spec or audit into a dispatchable, trackable set of tickets:

1. **Emit the batch in one operation.** Write the tickets as a JSON array or a TOML `[[ticket]]` manifest (each with id, `depends_on`, `related`, `scopes`, `tags`, and a `body` or `template`) and `ticketsplease create --from manifest.toml` — validated all-or-nothing, idempotent on re-run. Tag every ticket with the initiative key (e.g. `--tag m1`, or a `tags` field per spec) so the group can be rolled up. Use `--depends-on` only for true ordering; `--related` records a non-blocking cross-reference the scheduler ignores.
2. **Track where it stands.** `ticketsplease rollup --tag m1` → counts by status/priority, % done, the ready frontier (what to dispatch next within the initiative), and the blocked set with each ticket's unmet deps.
3. **Plan the shape.** `ticketsplease graph --tag m1 --dot | dot -Tsvg` visualizes the DAG; `ticketsplease path <id>` prints the longest prerequisite chain (critical path) to any ticket.
4. **Save the view.** `ticketsplease view save open-m1 'tag:m1 AND NOT status:done'`, then `list --view open-m1` / `rollup --view open-m1` is the reusable "epic view" (stored in the committable `.ticketsplease/views.toml`).

Then dispatch the ready frontier with the fan-out loop below.

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

## Tuning how parallel you go

The default is strict: two tickets never run together if *either* exclusively claims a scope they share (`scopes`). That's safe but over-serializes when the overlap is benign (e.g. both only *append* to a hub crate). The knobs, in the order you'll reach for them:

1. **Declare additive intent.** Put a scope in `shared_scopes` (vs `scopes`) when a ticket only appends/extends it. Two shared claims on a scope run in parallel; an exclusive (rewrite) claim still conflicts. This is the precise lever and it carries through to the guard, which won't fail a shared-by-both collision (`cause: shared`).
2. **Weight scopes once.** `[scope_policy]` in `ticketsplease.toml` sets a per-scope clash cost (`weight = 0` = a free-to-co-edit hub; higher = riskier) — set-and-forget instead of annotating every ticket.
3. **Tolerate a budget per dispatch.** `tracks`/`next`/`lanes --max-overlap K` co-schedules pairs whose clash cost is ≤ K (`any` = unbounded), filling N workers least-cost-first; the residual `overlap_cost` is reported so you can judge it. Pair with `guard --ignore-transitive` (and the additive-by-intent pass-through) so the merge gate agrees.
4. **Size the fleet.** `tracks --width` (also in `next`/`rollup` JSON) is the largest set safely runnable at once under the current budget — how many workers to spin up.
5. **Sequence instead of dropping.** `ticketsplease lanes --parallel N` plans ordered per-worker queues: conflicting tickets are chained onto one lane (later rebases on earlier) with a merge order, so no worker idles waiting for a recompute.
6. **Stay compatible mid-loop.** `next --running <ids>` (or, by default, the live-claimed in-progress set) drops picks that would clash with work already in flight — the right call when a single worker frees up.

Escape hatches when you'd rather not annotate: `--assume-shared` (treat everything additive — pack it all, reconcile at merge), `--strict` (ignore `shared_scopes`/weights — the conservative view), and `tracks --overlap-matrix` (the raw weighted conflict graph, so an external orchestrator assigns work itself).

## Branch naming

Name each branch so the guard can infer the ticket without `--ticket`: include the id, e.g. `tkt/<id>` or `<id>-short-description`. The guard picks the longest ticket id that appears in the branch name. When in doubt, pass `--ticket <id>` explicitly.

## The merge gate

A worker must pass the guard before its branch merges:

```
ticketsplease guard tkt/<id> --base main --format json
case exit:
  0 -> pass — but read `severity`. "ok" is clean; "warn" means a declared-area overlap with an
       open sibling was reported but does NOT gate. An overlap is the expected state under parallel
       dispatch, not a proven merge conflict — glance at `collisions` and coordinate if a sibling
       truly rewrites the same area, but it does not block this merge.
  6 -> CONFLICT — read the JSON (commands.md has the full guard schema):
         under_declared -> a genuine scope escape. Narrow the diff back, or if the area truly
             belongs to this ticket `ticketsplease set <id> --add-scope <scope>` (or add the file
             to `paths`) and re-guard. The cargo reverse-dep expansion never lands here, so editing
             a foundational crate within your declared globs will not trip it.
         collisions (only when you opted in with `--strict` or `[guard] gate_collisions`) -> a
             declared-area overlap you chose to gate. `cause: direct` = a real overlap, coordinate
             (merge the other first, or split the work); `cause: transitive` = reverse-dep-only —
             `guard --ignore-transitive` waves it through while still reporting it.
  other -> a setup problem (4 = no ticket resolved, 3 = bad input); fix and retry.
```

The guard's hard gate is **under-declaration** (a scope escape) — exit 6, always. A **declared-area overlap** with an open sibling is a non-failing **WARN** by default: under parallel dispatch it is the normal state, so it is reported (glance at it, coordinate if needed) but does not block the merge — keeping the exit-6 signal meaningful for the escape it is meant to catch. Opt an overlap into gating with `--strict` or `[guard] gate_collisions = true` when you want the stricter net. The guard reads the `[scopes]`/`[guard]` config from `--base` (not the checked-out branch, which may carry a stale/empty config), and reads sibling tickets' in-flight status from their `tkt/*` branch tips — so an overlap against a worker who is `in-progress` on its own branch is reported even when your checkout still shows that sibling as `todo`. Override the config source with `--config-ref` and the branch namespace with `--prefix`.

## Observing workers mid-flight

Workers advance status on their own `tkt/<id>` branches, so an orchestrator on `main` can't see it via `list` (working-tree only) until merge. Three commands read across branches without a checkout:

- `ticketsplease events --watch --since <cursor>` — the **multiplexed** wake-on-event across *all* tickets at once: returns the moment any worker changes status, claims, releases, or comments. Events live in `.git` refs, so you see them **without waiting for a commit** (unlike `status`/`show --ref`, which read committed branch state). Loop it, advancing `--since` to the last id you saw, to consume the stream without missing a transition. Prefer this to spawning N single-ticket watchers.
- `ticketsplease comment add <id> --as <w> --body -` / `comment list <id> --ref tkt/<id>` — leave durable notes on a ticket (blocked-reasons, decisions, questions) and read a worker's notes from `main`. Each `comment add` also rings the event doorbell above.
- `ticketsplease status --all-branches` — every `tkt/*` branch's ticket status at its committed tip; a simple snapshot when you don't need the live stream.
- `ticketsplease reconcile` — diff the board against git: in-progress tickets with no branch **and no live claim lease** (a dispatch that never started or was abandoned — a freshly-claimed ticket still holds its lease and is *not* flagged, so this stays clean during normal claim-then-branch until a lease actually expires), live `tkt/*` branches whose ticket still reads todo/ready (work the board doesn't reflect), and orphan branches. The board and git drift independently; run this before each dispatch round (exit 3 on drift) so you can trust `tracks`/`ready` before fanning out.
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

---

For exact flag syntax and JSON key names, see `references/commands.md`.
