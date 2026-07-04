---
name: ticketsplease
description: Manage and distribute development work across a codebase — break work into git-native markdown tickets, dispatch conflict-free parallel batches to multiple agents without merge collisions, query and roll up an initiative's progress, and verify a branch stayed inside its declared scope before merging. Backed by the ticketsplease CLI (`ticketsplease`, alias `tkt`). Use this whenever the user is coordinating work in a repo that has a `ticketsplease.toml` and a `tickets/` directory — deciding what to work on next, splitting work across several agents, creating/updating/linking/filtering tickets, rolling up an epic's status, or checking whether a branch's diff stayed inside its declared scope before merging. Reach for this skill whenever the user mentions tickets, parallel work, agent coordination, work distribution, what to work on next, conflict-free batches, dependency-ordered work, an initiative or epic's status, or guarding/validating a branch — even if they do not name ticketsplease explicitly.
allowed-tools: Bash, Read, Write, Grep, Glob
---

# ticketsplease — git-native parallel-work ticketing

ticketsplease (CLI `ticketsplease`, short alias `tkt`) manages development work as **git-versioned markdown tickets** and computes **conflict-free parallel work assignment** — so work can be split across multiple agents that never edit the same area of the codebase at once. Tickets are plain markdown + YAML frontmatter under `tickets/`; **scopes** (abstract area names like `core` or `query/planner`) are defined in `ticketsplease.toml` and map to file globs — and, for Rust repos, to crates.

## Why this exists

ticketsplease prevents the parallel-work failure mode — two agents editing the same files and colliding at merge — two ways. `tracks` partitions ready work into batches where no two tickets share a scope, so an entire batch is safe to run in parallel. `guard` checks a branch's *actual* diff against its ticket's *declared* scope — failing if the branch wandered into an area it never claimed, or into one another open ticket owns. For Rust repos the guard maps the diff through the cargo crate graph, so a change to a leaf crate is also checked against everything that depends on it.

## The contract — rely on this, not on prose

- Every command takes `--format json` for a stable, versioned payload. Human-readable text is the default; **pass `--format json` whenever you parse output.**
- **Exit codes are the API.** Gate on them:

  | code | meaning |
  |------|---------|
  | 0 | ok |
  | 2 | usage error (bad flags) |
  | 3 | invalid / dirty (malformed ticket, failed lint) |
  | 4 | ticket not found |
  | 5 | dependency cycle |
  | 6 | **conflict** — guard found a declared-area overlap (an under-declared scope, or collision with an open ticket); also a lost claim race, a held-claim release, or a `why` conflict. A conservative signal, *not* a proof of merge conflict. |
  | 7 | **timeout** — `watch` / `events --watch` gave up after `--timeout` seconds. |

- Output is deterministic (sorted, no timestamps) — safe to diff and cache.
- Every command accepts `--repo <path>` (default the current directory). Operations are fully offline and atomic.

## Setup

Locate the binary (`ticketsplease` or `tkt` on `PATH`; if neither is present, tell the user how they installed it or ask). Confirm the repo is initialized — there is a `ticketsplease.toml` at its root. If not:

```sh
ticketsplease init        # scaffolds tickets/ + ticketsplease.toml; links the skill into .claude/skills (a gitignored symlink to the canonical per-user copy)
ticketsplease guide       # the conceptual model in one screen (scopes, tracks, scoring, guard, claims)
ticketsplease doctor      # verify setup: config, git repo + commit, scope globs, base ref, skill freshness
```

After upgrading the binary, the installer refreshes the canonical skill; if `doctor` flags it as stale, run `ticketsplease skill sync` (and `ticketsplease migrate` to repair a project link). See `references/commands.md` → `skill`.

Then edit `ticketsplease.toml`: define `[scopes]` (name → globs) for the areas of the codebase. For a Rust repo, set `[language] backend = "rust"` and map `[scope_crates]` (scope → crate) so the guard can expand reverse-dependents (collisions from that expansion are tagged `transitive` so you can triage them; `guard --direct-only`, or `[language] reverse_dep_expansion = false` for a repo default, skips it). Under-declaration is always file-based, so this expansion never causes a false "out of scope" on a shared foundational crate. Use `[external_scopes]` (name → `{ repo, paths }`) to name a forked dependency pinned via `git = … rev = …` so the guard flags a branch that bumps its pin.

## The orchestration loop (dispatching parallel work)

1. **Get the conflict-free batches:**
   ```sh
   ticketsplease tracks --format json
   ```
   Each element of `batches` is a set of tickets that share no scope. Dispatch all members of a single batch to separate workers at the same time — they are guaranteed not to collide.

2. **Per worker, claim the ticket first** — the atomic hand-off that stops two workers grabbing the same one:
   ```sh
   ticketsplease claim <id> --as <worker-id> --format json   # exit 6 → already claimed, pick another
   ```
   The claim is race-safe and lease-backed (exactly one of N racers wins; the rest get exit 6; a crashed worker's ticket becomes reclaimable), refuses a ticket whose dependencies aren't all done, and flips it to in-progress so `ready`/`tracks`/`next` stop offering it — this is what makes **pull-based** dispatch safe with no central coordinator. On a clean claim, branch with the ticket id in the name (e.g. `tkt/<id>`) and work only inside its declared scope. `next --claim --as <worker>` collapses recommend-then-claim into one atomic call; `claims` shows who holds what; `claim --force` steals a live lease. See `references/parallel-workflow.md` for the push-vs-pull pattern and lease recovery.

3. **Before merging, guard the branch:**
   ```sh
   ticketsplease guard tkt/<id> --format json   # exit 6 → do not merge
   ```
   - Exit `0` → the diff stays within the declared scope and overlaps no open ticket's *exclusively* claimed area; it clears this **pre-merge filter**. (A shared-by-both collision is still reported in the JSON but does not block — see access intent. This is a partitioning check, not a substitute for your normal build/test gate — disjoint branches can still conflict semantically.)
   - Exit `6` → a **declared-area overlap, not a proven conflict** — the JSON says where: `under_declared` (scopes whose files the branch edited but that fall outside its declared area; file-authoritative) and `collisions` (open tickets whose area the diff overlaps, each tagged `direct` = a real overlap, `transitive` = reverse-dep-only, or `shared` = both additive and non-gating). To gate on the exit code without parsing JSON, run `guard --ignore-transitive` (exits 0 unless there's a direct overlap or under-declaration). See `references/parallel-workflow.md` for the resolution playbook (`set --add-scope`, coordinate with the named ticket, or build+test the merge) and the `--direct-only` flag.

4. **Finish, release, or close.** `claim` already set the ticket in-progress; on completion move it forward with `ticketsplease set <id> --status review|done` (a terminal status — `done` or `closed` — clears the claim). If you're stepping away but the work still stands, `ticketsplease release <id> --as <worker-id>` drops the claim and restores the pre-claim status (keeping any progress you'd advanced to). If the work itself should *not* be done — won't-do, duplicate, obsolete, superseded — `ticketsplease close <id> --reason <r> [--note <t>]` terminates it without completing: unlike `done` it does **not** satisfy dependents (they become *orphaned*, surfaced for re-pointing rather than silently unblocked), and `ticketsplease reopen <id>` undoes it. Renew a long-running claim by re-running `claim` (it extends your lease; a renewal logs no duplicate event).

5. **Observe and coordinate in flight.** Workers advance status and leave notes on their own `tkt/<id>` branches; from `main` you watch the shared activity log without a checkout:
   ```sh
   ticketsplease events --watch --since <cursor> --format json   # wake on the next status/claim/comment, across all tickets
   ticketsplease comment add <id> --as <worker> --body-file -     # leave a durable note (e.g. a blocked-reason)
   ```
   Events are `.git` refs, so they're visible the instant they're written — no commit needed — and `comment add` rings the same doorbell. Comments are append-only files (one per comment), conflict-free under concurrent authors. `tkt show <id>` folds a ticket's comments in. See `references/parallel-workflow.md` for the full observe/coordinate loop.

For a single highest-leverage pick instead of a whole batch:
```sh
ticketsplease next --format json               # one ticket
ticketsplease next --parallel 4 --format json  # 4 mutually conflict-free picks
```

## Picking and inspecting work

- `ticketsplease ready` — dependency-satisfied tickets, priority-ordered (a ticket is ready when its status is todo/ready and every dependency is **done**). A dependency that was `closed` (terminated without completing) rather than done does *not* satisfy — the dependent is left **orphaned** (surfaced by `rollup`/`lint`, never silently dispatched), so re-point, waive, or close it.
- `ticketsplease tracks [--max-overlap K] [--width]` — conflict-free parallel batches (the headline feature); `--width` shows how many workers are safe to run right now. Overlap tuning: see **Access intent & overlap** below.
- `ticketsplease lanes [--parallel N]` — ordered per-worker queues that *sequence* conflicting work onto one lane instead of dropping it (with a merge order). Use when you want a full N-worker plan, not just the immediate front.
- `ticketsplease why <a> <b>` — explain whether two tickets can co-run, and if not, the exact reason (a shared scope, or one transitively depends on the other). Use it when the scheduler's grouping is surprising.
- `ticketsplease next [--parallel N] [--max-overlap K] [--running ids]` — scored recommendation(s) favouring priority, critical-path position, and downstream unblock count; `--running` keeps picks compatible with in-flight work, `--claim --as <w>` claims one atomically. (Flag semantics: `references/commands.md`.)
- `ticketsplease list [--status <s>] [--where '<expr>']` — list/filter tickets. `--where` is a boolean expression: `field:value` joined by `AND`/`OR`/`NOT` with parens (fields: status priority tag scope assignee id dep related reason), e.g. `--where 'tag:epic AND NOT status:done'` or `--where 'status:closed AND reason:duplicate'`. `ticketsplease view save <name> '<expr>'` stores one as a reusable view (then `list --view <name>`). `ticketsplease show <id>`.
- `ticketsplease rollup [--tag <t> | --where <e> | --view <v>]` — an initiative's dashboard: status & priority counts, % done (plus a `closed` count), the ready frontier, the blocked set (each with its unmet deps), and any tickets **orphaned** by a closed dependency. The one-call "where does this epic stand and what's next in it".
- `ticketsplease states` — the workflow's states and their engine **category** (dispatchable/open/parked/terminal). A repo keeps the built-in states unless it declares `[workflow.states]`; custom states are named freely but pin to a category the scheduler/guard/rollup reason about (see `references/commands.md`).
- `ticketsplease graph [--tag <t>] [--dot]` and `ticketsplease path <id>` — export the dependency DAG (JSON, or Graphviz with `--dot`) and the critical prerequisite path (longest dependency chain) to a ticket.
- `ticketsplease reconcile` — cross-check the board against git (tkt/* branches + worktrees): flags in-progress tickets with no branch (stale-busy) and live branches whose ticket is still todo/ready (stale-idle), plus orphan branches. Exit 3 on drift. Run it before a dispatch round so the board can be trusted.

## Access intent & overlap (tuning parallelism)

Claim a scope in one of two modes so you needn't single-thread on benign clashes: `scopes` is an **exclusive** (rewrite) claim, `shared_scopes` is a **shared/additive** (append/extend) one. Two shared claims on a scope co-run; any exclusive claim blocks any other claim on that scope. On top of that, `--max-overlap K` on `tracks`/`next`/`lanes` tolerates clashes up to a per-pair cost budget so you fill N workers least-riskily instead of idling them.

For the full tuning playbook — scope weights (`[scope_policy]`), the escape hatches, sequencing conflicts with `lanes`, and how the guard honours all of it — read `references/parallel-workflow.md`.

## Creating and editing tickets

```sh
ticketsplease create --title "Add vector index" --priority p1 \
  --scope query/planner --shared-scope changelog --depends-on build-index-trait --related design-doc --template default
ticketsplease create --from backlog.toml    # batch from a JSON array or TOML [[ticket]] (- reads stdin); all-or-nothing
ticketsplease set <id> --status in-progress --add-scope core --add-dependency other
ticketsplease set --where 'tag:epic' --add-tag ready-soon   # bulk-edit every match (field edits only, not title/body)
ticketsplease link <id> (--depends-on <o> | --related <o>)  # depends-on cycle → exit 5; related is non-blocking, never cycle-checked
ticketsplease rename <old> <new>            # moves the file, rewrites the id, repoints dependents
ticketsplease delete <id>                   # remove a ticket (git keeps history)
ticketsplease lint        # validate schema, scope refs, links, and cycles (exit 3 / 5 on problems)
```

`dependencies` block scheduling; `related` is a soft, non-blocking "see also" (queryable via `--where related:x`, ignored by `ready`/`tracks`). `--template <name>` scaffolds the body from `.ticketsplease/templates/<name>.md` (seeded by `init`). Add `--dry-run` to `create`/`set` to preview without writing. Edits are **round-trip-safe**: ticketsplease rewrites only the field it changes and leaves everything else — custom frontmatter keys, comments, key order, and the markdown body — byte-for-byte. You can also hand-edit ticket files directly; they are just markdown.

For the full frontmatter schema and per-field semantics, see `references/commands.md`. One safety rule: claiming manages `assignee` and `lease_expires_at` (epoch seconds) — leave those to `claim`/`release` rather than hand-editing them.

## Deeper references

- **`references/commands.md`** — read when you need exact flag syntax, JSON key names, or a command's per-exit-code behaviour.
- **`references/parallel-workflow.md`** — read when setting up a multi-worker fan-out, tuning how much parallelism to allow (`--max-overlap`, `[scope_policy]`, `lanes`, escape hatches), authoring/organizing an initiative (batch create, tag, `rollup`, `graph`), diagnosing a guard failure, or handling cross-clone propagation.
