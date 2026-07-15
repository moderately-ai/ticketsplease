# ticketsplease command reference

Global flags (accepted by every command):

- `--repo <path>` — repository root (default `.`).
- `--format human|json` — `human` is the default; `json` is the stable, versioned contract. Every JSON payload includes `"schema_version": 1` and is deterministically ordered.

Exit codes are the contract — see the table in `SKILL.md` (`0` ok · `2` usage · `3` invalid · `4` not found · `5` cycle · `6` conflict · `7` timeout).

## Contents

`init` · `create` · `set` · `close` / `reopen` · `link` · `show` / `list` · `view` · `rollup` · `graph` / `path` · `status` · `reconcile` · `watch` · `comment add` / `list` · `events` · `ready` · `tracks` · `lanes` · `next` · `why` · `run` · `claim` / `release` · `guard` · `delete` / `rename` · `doctor` / `guide` · `states` · `lint` · `skill install` / `sync` / `self-update` (each is a `##` section below; the conventions that follow apply to all).

## Conventions

- **Result key per command.** Each command's payload carries its result under a stable, documented key, listed with the command below. The quick map: `init`→(fields) · `create`→`results` · `set`→(fields, or `results` in bulk) · `close`/`reopen`→(fields) · `link`→(fields) · `show`→(fields) · `list`→`tickets` · `view`→(fields/`views`) · `rollup`→(fields) · `graph`→`nodes`/`edges` · `path`→`path` · `status`→`tickets` · `reconcile`→`findings` · `claims`→`claims` · `ready`→`ready` · `tracks`→`batches` (or `matrix`/`width`) · `lanes`→`lanes`/`merge_order` · `next`→`picks` (or `claimed` with `--claim`) · `why`→(fields) · `run`→`outputs` (or `steps` with `--dry-run`) · `guard`→(fields) · `lint`→`diagnostics` · `comment list`→`comments` · `events`→`events` · `doctor`→`checks` · `guide`→`guide` · `delete`/`rename`→(fields).
- **`id` vs `ticket`.** When an object *is* a ticket (show/list/ready/status/claims), its id is `id`. When an object *references* a ticket from elsewhere (a comment, an event, a collision, a `conflicts_with` entry), the referenced ticket is `ticket` and the object's own id (if any) is `id`/`comment_id`. So `id` is always "this object", `ticket` is always "the ticket it's about".
- **`depends_on` in, `dependencies` out.** Inputs that accept dependencies use `depends_on` (`create --depends-on`, `link --depends-on`, and the batch spec key, which also accepts `dependencies` as an alias). Stored/queried output always uses `dependencies`.
- **`dependencies` block; `related` does not.** `dependencies` gate scheduling (a ticket is not `ready` until all are `done`) and are cycle-checked. `related` is a soft, non-blocking cross-reference: recorded, queryable (`--where related:x`), and graphable, but ignored by `ready`/`tracks`/`next`/cycle-detection. Use `related` for "see also", `depends_on` for "must finish first".
- **Access intent: `scopes` exclusive, `shared_scopes` additive.** A scope in `scopes` is an *exclusive* (rewrite) claim; one in `shared_scopes` is a *shared* (additive/append) claim. Two tickets that both hold a scope shared are compatible (run in parallel); a shared claim still conflicts with an exclusive one. `[scope_policy]` in `ticketsplease.toml` sets a per-scope conflict-cost `weight` (default 1; `0` = free to co-edit). `tracks`/`next`/`lanes` gate on a per-pair **overlap budget** (`--max-overlap K`, `0`=strict … `any`=unbounded), so you can fill workers least-riskily instead of single-threading. The guard tags a shared-by-both collision `cause: shared` and does not fail on it.

## init

```
ticketsplease init [--dir tickets] [--force] [--no-skill] [--harness claude|codex|opencode|pi-agent]
```
Scaffolds `<dir>/` and `ticketsplease.toml`, links the bundled skill into the chosen harness's skills directory (default `--harness claude` → `.claude/skills/ticketsplease`; a gitignored symlink to the canonical copy — see `skill`), and seeds example body templates into `.ticketsplease/templates/` (for `create --template`) and example recipes into `.ticketsplease/recipes/` (for `tkt run` — see `run`). `--no-skill` skips the skill link. Idempotent: an existing config is left untouched unless `--force`. Prints a next-steps block, and warns if the directory is not a git repo (claim/guard/status/events/watch need `git init` + a commit).

JSON: `{ "schema_version", "tickets_dir", "wrote_config", "skill_installed", "templates_installed", "recipes_installed", "git": bool }`.

## create

```
ticketsplease create --title <s> [--id <slug>] [--status <s>] [--priority p0..p3]
                      [--depends-on a,b] [--related c,d] [--scope x,y] [--shared-scope z]
                      [--path 'glob'] [--tag t] [--body <s>] [--template <name>] [--dry-run] [--no-validate]
ticketsplease create --from <file|-> [--dry-run] [--no-validate]
```
Writes new tickets atomically. Without `--id`, the id is a slug of the title and the create is **content-addressed-idempotent**: re-running the same create is a no-op (`created: false`), not a `<slug>-2` clone; a genuinely different ticket at that slug takes the next suffix. With `--id`, re-running with identical content is a no-op; different content with the same id is an error (exit 3).

`--template <name>` scaffolds the body from `.ticketsplease/templates/<name>.md` (seeded by `init`; add your own), substituting `{{title}}` and `{{id}}`. An explicit `--body` wins over `--template`; an unknown template is exit 4.

`--from` batch-creates from a **JSON array** of specs or a **TOML `[[ticket]]`** document (format chosen by `.json`/`.toml` extension; `-` reads stdin, defaulting to JSON unless the content starts with `[[`). Each spec is `{title, id?, status?, priority?, depends_on?, related?, scopes?, shared_scopes?, paths?, tags?, body?, template?}`. Unknown keys are **rejected** (a typo like `dependson` fails loudly). The whole batch is validated before any write (a bad element aborts before partial state). `--dry-run` previews without writing.

**Write-time validation (default on).** `create` rejects (exit 3) a declared **scope not defined** in `ticketsplease.toml` (`[scopes]`/`[scope_crates]`/`[external_scopes]`) and a **dangling `related`/`depends_on`** id, so a bad batch fails at filing rather than surfacing later at `lint`/`guard`. A `--from` batch resolves cross-references among its own members, so a self-consistent batch passes. Pass **`--no-validate`** for a forward reference to a ticket or scope you will add next. (Same checks apply to `set`/`link` — see below.)

JSON (single and batch share one shape): `{ "schema_version", "results": [ {id, created: bool, path} ], "dry_run": bool }`.

## set

```
ticketsplease set (<id> | --where <expr> | --view <name>)
                       [--title <s>] [--status <s>] [--priority <p>]
                       [--add-scope a,b] [--remove-scope c] [--add-tag t] [--remove-tag u]
                       [--add-shared-scope z] [--remove-shared-scope w]
                       [--add-path 'glob'] [--remove-path 'glob']
                       [--add-dependency d] [--remove-dependency e]
                       [--add-related r] [--remove-related s]
                       [--body <s> | --body-file <f|-> | --append-body <s> | --append-body-file <f|->] [--dry-run] [--no-validate]
```
Surgically updates fields (round-trip-safe), writing back to the file it read even if the frontmatter `id` has drifted from the filename. No-op if nothing changes. `--add-dependency` is rejected if it would close a cycle (exit 5), like `link`; `--add-related` is never cycle-checked. An **added** scope that is undefined, or an added `related`/`depends_on` id that does not resolve, is rejected (exit 3) — only the additions are checked, so an unrelated edit never trips on a pre-existing dangling link; `--no-validate` skips it. Setting a terminal status (`done` or `closed`) clears the claim (assignee + lease). `--reason <duplicate|wontdo|obsolete|superseded|cancelled>` and `--note <text>` are valid only alongside `--status closed` (they record the resolution, and are cleared automatically when the ticket later leaves `closed`); prefer the `close`/`reopen` verbs below. When `[workflow] enforce_transitions` is on, a status change that is not a permitted transition is rejected (exit 6) unless `--force` is passed (bulk `--where` skips illegal ones instead). `--dry-run` previews without writing.

**Single vs bulk:** pass an `id` to edit one ticket, or `--where`/`--view` to edit **every matching ticket** in one operation (exactly one of the two; passing both, or neither, is exit 3). Bulk applies field edits only — `--title` and the body edits are single-target and rejected with `--where`/`--view`. A single cycle check runs over the whole edited set after all dependency edits.

Single JSON: `{ "schema_version", "id", "changed": bool, "dry_run": bool }`.
Bulk JSON: `{ "schema_version", "matched": N, "results": [ {id, changed: bool} ], "dry_run": bool }`.

## close / reopen

```
ticketsplease close  <id> [--reason <duplicate|wontdo|obsolete|superseded|cancelled>] [--note <text>] [--dry-run]
ticketsplease reopen <id> [--status <active-status>] [--dry-run]     # default --status todo
```
`close` terminates a ticket **without** completing it — the terminal counterpart to `done`. Like `done` it is excluded from scheduling and drops any claim, **but it does not satisfy dependents**: a ticket that depends on a closed one is *orphaned* (listed by `rollup`, failed by `lint`, and refused by `claim` with a pointed message) so you re-point it, waive the dependency (`set --remove-dependency`), or close it too — it is never silently dispatched onto abandoned work. The optional `--reason` (a small fixed vocabulary) and `--note` are stored in frontmatter (`closed_reason`/`closed_note`) and echoed on the status event; query them with `list --where 'reason:duplicate'`.

`reopen` returns a terminal (closed **or** done) ticket to an active status and **clears `closed_reason`/`closed_note` in the same write** — the resolution never lingers to contradict the live status (the prior reason survives in the activity log / git history). Reopening a non-terminal ticket, or into a terminal target, is exit 3. `close` is sugar for `set --status closed --reason … --note …`; `reopen` is the atomic clear-on-transition that raw `set` can't express as cleanly.

JSON (both): `{ "schema_version", "id", "changed": bool, "dry_run": bool }`.

## link

```
ticketsplease link <id> (--depends-on <other> | --related <other>) [--remove] [--no-validate]
```
Adds (or with `--remove`, removes) a link. `--depends-on` is a hard, cycle-checked **dependency** edge; `--related` is a soft, non-blocking cross-reference that scheduling ignores (and so is never cycle-checked). Exactly one of the two is required. A dangling target is **rejected** at write time (exit 3) unless `--no-validate` — consistent with `create`/`set` (`--no-validate` allows a forward reference, which `lint` then reports as `missing-dep`/`missing-related`). A dependency edge that closes a **cycle** is rejected regardless (exit 5). `--remove` never validates the target, so a link to a deleted ticket can be cleaned. A self-link is rejected (exit 3).

JSON: `{ "schema_version", "id", "depends_on"|"related", "removed", "changed" }`.

## show / list

```
ticketsplease show <id> [--ref <branch>]
ticketsplease list [--status <s>] [--scope <s>] [--tag <t>] [--priority <p>] [--where <expr>] [--view <name>] [--hide-done]
```
`show --format human` prints a rendered field view + body + comments (including the close reason/note on a closed ticket); `--format json` → the ticket's fields (`closed_reason`/`closed_note` included). `--ref` reads the ticket as committed on a git ref (no checkout). `list` filters compose (AND); `--hide-done` drops terminal tickets (`done` + `closed`). A malformed ticket file degrades to a warning rather than failing the listing.

`--where` is a boolean filter expression: `field:value` terms joined by `AND` / `OR` / `NOT` (case-insensitive) with parentheses; it composes (AND) with the single-axis flags. Fields: `status`, `priority`, `tag`, `scope`, `assignee`, `id`, `dep`, `related`, `reason`. Values are barewords (`p0`, `query/planner`, slug ids) or quoted (`"needs review"`). `status:`/`priority:`/`reason:` values are validated, so a typo exits 3. Examples: `--where 'tag:dialect AND NOT status:done'`, `--where 'status:closed AND reason:duplicate'`, `--where '(priority:p0 OR priority:p1) AND scope:core'`. `--view <name>` applies a saved expression and ANDs with `--where`.

## view

```
ticketsplease view save <name> <expr>     # store/overwrite a named --where expression (validated)
ticketsplease view list                   # all saved views
ticketsplease view show <name>            # print one view's expression
ticketsplease view delete <name>
```
Saved views live in `<repo>/.ticketsplease/views.toml` — a tool-owned, **committable** file (a shared "epic view"), separate from `ticketsplease.toml`. `save` validates the expression (a bad one exits 3, like `--where`); `show`/`delete` on an unknown name exit 4. Apply a view with `list --view <name>` (and `set --where`/`rollup` accept `--view` too).

view JSON: `save` → `{ "schema_version", "name", "where", "replaced" }`; `list` → `{ "schema_version", "views": [ {name, where} ] }`; `show` → `{ "schema_version", "name", "where" }`; `delete` → `{ "schema_version", "name", "deleted" }`.

## rollup

```
ticketsplease rollup [--tag <t>] [--where <expr>] [--view <name>]
```
Aggregates an initiative (a tag and/or filter; selectors AND together — no selector = the whole board): status & priority counts, percent done, the **ready frontier**, and the **blocked set**. Readiness is computed over the *full* board (so a prerequisite outside the selection still counts) and then intersected with the selection; `blocked` is the selected dispatchable-status tickets that have an unfinished dependency, each with the unmet ids. Use it to answer "where does this initiative stand and what's next in it" in one call.

JSON: `{ "schema_version", "selector": {tag,where,view}, "total", "done", "percent_done", "width", "by_status": {status: n}, "by_priority": {p: n}, "ready": [ {id,title,priority} ], "blocked": [ {id,title,unmet: [ids]} ] }` (`width` = safe parallel width within the ready frontier).

## graph / path

```
ticketsplease graph [--tag <t>] [--where <expr>] [--view <name>] [--dot]
ticketsplease path <id>
```
`graph` exports the dependency DAG: nodes carry the same scoring metrics `next` ranks by (`score`, `critical_path`, `downstream_count`), edges are dependencies, and `related_edges` are the non-blocking links. Selectors restrict the emitted subgraph (induced — an edge is kept only when both endpoints are selected); metrics stay board-global. `--dot` emits Graphviz (`dot -Tsvg`) with dependency edges solid and related edges dashed.

`path <id>` prints the **critical prerequisite path** — the longest chain of dependencies that must complete before `<id>` — root-first, each step with its status. The longest pole to finishing a ticket.

graph JSON: `{ "schema_version", "nodes": [ {id,title,status,priority,score,critical_path,downstream_count} ], "edges": [ {from,to} ], "related_edges": [ {from,to} ] }`.
path JSON: `{ "schema_version", "id", "length", "path": [ {id,status,title} ] }` (exit 4 if the id is unknown).

list JSON: `{ "schema_version", "tickets": [ {id,title,status,priority,scopes,paths,dependencies,tags} ], "warnings": [...] }`.

## status

```
ticketsplease status [--all-branches] [--prefix tkt/]
```
Without flags, the working-tree status of every ticket. `--all-branches` scans `refs/heads/<prefix>*` and reports each ticket's status as committed on its branch tip (a branch whose ticket file is absent on its tip is reported with `status: null`). JSON: `{ "schema_version", "source": "worktree"|"branches", "tickets": [ {branch?, id, status, assignee, lease_expires_at} ] }`.

## reconcile

```
ticketsplease reconcile [--prefix tkt/]
```
Cross-checks each ticket's status against git reality — the `<prefix>*` work branches and `git worktree list` — and reports where the board has drifted (ticket status lives in markdown with no link to whether a branch/worktree actually exists). Findings:
- `in-progress-no-branch` — a ticket in an open state with no work branch **and no live claim lease** (abandoned dispatch, or a manual `set --status`; **stale-busy**). A ticket freshly claimed but not yet branched holds a live lease, so it is treated as legitimately mid-dispatch and **suppressed** — until the lease expires (default 1h), when a genuinely abandoned dispatch resurfaces. This keeps `reconcile` clean during normal claim-then-branch orchestration.
- `branch-without-active-ticket` — a `<prefix><id>` branch exists but the ticket is still todo/ready (untracked in-flight work; **stale-idle**).
- `orphan-branch` — a `<prefix>*` branch with no matching ticket.

Each finding carries `worktree: bool` (a worktree is checked out on that branch). **Exit 3** when any drift is found (so `reconcile && dispatch` gates), `0` when the board matches git. JSON: `{ "schema_version", "ok": bool, "findings": [ {id, issue, status, branch: bool, worktree: bool, detail} ] }`.

## claims

```
ticketsplease claims [--all-branches] [--prefix tkt/]
```
Who holds what: every claimed ticket with assignee, `lease_expires_at`, and `live` (lease still valid). `--all-branches` overlays `<prefix>*` branch tips. JSON: `{ "schema_version", "claims": [ {id, assignee, lease_expires_at, live: bool, status} ], "warnings": [...] }`.

## watch

```
ticketsplease watch <id> --until <status> [--ref <branch>] [--prefix tkt/] [--interval 5] [--timeout <secs>]
```
Blocks until the ticket reaches `--until` (or `done`, always terminal), then exits 0. Without `--ref`, polls the `<prefix><id>` branch if it exists, else the working tree. **Exit 7** on `--timeout`. JSON (printed on both paths): `{ "schema_version", "id", "ref", "status", "reached": bool, "timed_out": bool }`.

## comment add / list

```
ticketsplease comment add <id> [--as <author>] [--reply-to <comment-id>] (--body <text> | --body-file <f|->)
ticketsplease comment list <id> [--ref <branch>]
```
`comment add` appends a comment as its own file under `<tickets_dir>/<id>.comments/<comment-id>.md` (one file per comment — concurrent authors never conflict). `--reply-to` must reference an existing comment id (else exit 4). The ticket must exist (else exit 4). `comment list` shows comments chronologically, replies nested under their parent (human) with relative timestamps; `--ref` reads them as committed on a branch. `tkt show <id>` folds comments in. JSON: `{ "schema_version", "ticket", "comments": [ {id, by, at, reply_to, body} ] }`. Adding a comment also emits an **event**.

## events

```
ticketsplease events [--since <event-id>] [--ticket <id>] [--type <kind>] [--watch] [--interval 2] [--timeout <secs>]
```
The cross-branch activity log: each event is a `refs/ticketsplease/events/<id>` ref pointing at a JSON blob in `.git`, visible across worktrees and a shared clone **immediately — no commit, no push** (but see cross-clone note in the parallel-workflow guide). `comment add`, `set --status`, `claim`, and `release` emit events. The id is time-sortable; `--since <last-seen-id>` is a resumable cursor. `--ticket` (must exist) / `--type` (one of `comment`, `status`, `claim`, `release`) are validated — a typo fails loudly rather than masking the stream. Requires a git repo (else a clean error, not silent empty). `--watch` blocks until a matching event appears, exiting **7** on `--timeout`. Human output shows relative timestamps. JSON: `{ "schema_version", "events": [ {id, ticket, kind, by, at, data} ] }`.

## ready

```
ticketsplease ready
```
Dispatchable tickets (status todo/ready with every dependency done), ordered by `(priority, id)`. A dependency cycle is a hard error (exit 5).

JSON: `{ "schema_version", "ready": [ {id,title,status,priority,scopes,paths,dependencies,tags} ] }`.

## tracks

```
ticketsplease tracks [--parallel N] [--max-overlap K] [--width] [--overlap-matrix]
                     [--assume-shared | --strict]
```
Partitions the ready set into batches; no two tickets in a batch conflict beyond the budget. Dispatch one batch fully in parallel. `--parallel N` caps each batch to N tickets (splitting larger ones), giving worker-sized fronts.

`--max-overlap K` is the per-pair overlap budget: `0` (default) = strictly conflict-free; `K` = let tickets that conflict by ≤ K per pair share a batch; `any` = unbounded. Each batch's residual `overlap_cost` is reported. `--width` prints only the **safe parallel width** (the largest set runnable at once within the budget) — how many workers to spin up. `--overlap-matrix` instead emits the raw conflict graph (every ready pair with conflicting scopes and cost) for self-service assignment. `--assume-shared` treats every claim as shared (collapse conflicts; reconcile at merge); `--strict` treats every claim as exclusive (ignore `shared_scopes` and weights).

JSON: `{ "schema_version", "batches": [ [ {id,...} ] ], "overlap_cost", "width" }`; with `--width`: `{ "schema_version", "width" }`; with `--overlap-matrix`: `{ "schema_version", "matrix": [ {a, b, scopes, cost} ], "width" }`.

## lanes

```
ticketsplease lanes [--parallel N] [--max-overlap K] [--assume-shared | --strict]
```
Plans **worker lanes**: ordered per-worker queues that *sequence* conflicting work onto one lane (the later rebases on the earlier) instead of dropping it to a future batch and idling a worker. `--parallel N` is the lane count (default: the safe parallel width); `--max-overlap` tolerates cheap overlaps within a concurrent round (same model as `tracks`). The merge order completes an earlier round everywhere before the next round's heads start.

JSON: `{ "schema_version", "lanes": [ [ {id,...} ] ], "merge_order": [ids] }`.

## next

```
ticketsplease next [--parallel N] [--max-overlap K] [--running ids] [--allow-overlap]
                   [--assume-shared | --strict] [--claim --as <worker> [--ttl <secs>]]
```
The highest-scored dispatchable ticket(s). **Score** = `1000 × priority (p0=3..p3=0) + 10 × critical-path length + count of not-done tickets it unblocks` — higher is more impactful. Picks fill in two passes: highest-scored compatible picks first, then — within `--max-overlap` (`0` default … `any`) — the lowest-cost overlaps to fill N, each annotated with `conflicts_with` (scopes + cost). `--allow-overlap` is the `--max-overlap any` alias. `--running <ids>` (alias `--avoid`) drops picks conflicting with those in-flight tickets; omit it to default to every in-progress ticket with a live claim (so a dispatch loop is in-flight-aware with no args). `--claim --as <worker>` atomically claims the first still-free pick (a lost race falls through to the next).

JSON: `{ "schema_version", "picks": [ {id,...,score, "conflicts_with": [ {ticket,scopes,cost} ]} ], "overlap_cost", "width" }`, or with `--claim`: a claim payload (see below) or `{ "schema_version", "claimed": null }` when nothing is free.

## why

```
ticketsplease why <a> <b>
```
Explains whether two *different* tickets can run in parallel (passing the same id twice is a usage error, exit 3). They cannot if they share a scope **or** one transitively depends on the other. JSON: `{ "schema_version", "a", "b", "conflict": bool, "shared_scopes": [...], "dependency_ordered": bool }`. Exits 6 on conflict (so `why a b && …` gates).

## run

```
ticketsplease run <name> [--arg key=value …] [--dry-run] [--list] [--describe]
```
Runs a **recipe**: a named, typed, parameterized procedure over these same subcommands. Recipes are registered as `[recipe.<name>]` in `ticketsplease.toml` **or** as a discovered `.ticketsplease/recipes/<name>.toml` file (filename = name; a name defined in both is a loud error, exit 3). A recipe is **not** a `[workflow]` (that is the lifecycle state machine) — it is a macro over commands, invoked explicitly. There are deliberately no event or scheduled triggers: a daemonless tool has no watcher, so invocation *is* the trigger. `init` seeds `supersede` and `split` as examples; `tkt run --list` shows what's available and `tkt run <name> --describe` prints one recipe's typed contract.

**Typed inputs, validated before any step runs** (so a bad input mutates nothing). Supply them with `--arg <name>=<value>`. Types: `string` (default), `int`, `bool`, `enum` (with `options`), `ticket`; each input may be `required`, carry a `default`, or be `multiple` (a comma-separated list). A missing-required / wrong-type / bad-enum / undeclared input is **exit 3**; a `ticket` input that does not resolve is **exit 4**.

**Steps are structured invocations, not shell.** Each `[[recipe.<name>.steps]]` table has a `command` (the subcommand), an optional `args` list (positionals), an optional `id` (so a later step can read its output), and any other key as a `--flag` — `add-dependency = "x"` → `--add-dependency x`, `force = true` → `--force`. Each step re-invokes this binary with `--format json`, so it inherits that command's validation, JSON, and exit code; a failing step **stops the recipe and propagates its exit code**. `{{inputs.<name>}}` substitutes an input; `{{steps.<id>.<dotted.path>}}` pulls a **scalar from a prior step's JSON** — the single cross-step data-flow primitive — and hard-fails (exit 3) if the path is absent (never a silent blank). There are intentionally **no conditionals or loops**: model divergence as separate recipes, and `--where` is the loop. For real logic, write a shell script that calls `tkt` and reads its JSON/exit codes.

`--dry-run` prints the resolved ordered plan (step references stay symbolic) and touches nothing — Read it before running a destructive recipe. There is no cross-step rollback: a recipe fails fast, and git is the undo.

`[recipe.<name>]` fields: `description`; `[recipe.<name>.inputs.<param>]` (`type` / `required` / `default` / `options` / `multiple` / `description`); `[[recipe.<name>.steps]]`; and `[recipe.<name>.outputs]` (`name = "<template>"`, resolved and projected into the result). JSON: `{ "schema_version", "recipe", "steps": N, "outputs": {…} }`; with `--dry-run`: `{ …, "dry_run": true, "steps": [ [argv…], … ] }`; `--list` → `{ "recipes": [ {name, description, inputs} ] }`.

Example (`supersede`, seeded by `init` — replace a ticket with N successors):
```
tkt run supersede --arg id=auth --arg with=auth-api,auth-ui,auth-db
  # 1. set --where 'dep:auth' --add-dependency auth-api,… --remove-dependency auth   (re-point every dependent)
  # 2. set auth --add-related auth-api,…                                             (keep a trail)
  # 3. close auth --reason superseded                                               (retire the original)
```

## claim / release

```
ticketsplease claim <id> --as <worker> [--ttl <secs>] [--force]   # default ttl 3600
ticketsplease release <id> [--as <worker>] [--force]
```
`claim` atomically takes a ticket (git-ref compare-and-swap on `refs/ticketsplease/claim/<id>`): of N racing workers, exactly one wins, the rest get **exit 6**. It records `assignee` + `lease_expires_at` (an unquoted integer) and marks the ticket in-progress, remembering the pre-claim status. An expired lease is reclaimable (`stolen: true`); `--force` steals even a *live* lease. Re-claiming as the holder is a `renewed` no-op (no duplicate event). A ticket is unclaimable if its status isn't todo/ready/in-progress (exit 6) **or** its dependencies aren't all done (exit 6).

`release` restores the pre-claim status (not always `ready`) — but keeps real progress if the worker advanced to review/blocked/done. Without `--force`, only the recorded holder may release; a **bare** `release` (no `--as`) on a held ticket is refused (pass `--as <holder>` or `--force`).

claim JSON: `{ "schema_version", "id", "assignee", "lease_expires_at", "stolen": bool, "renewed": bool }`.
release JSON: `{ "schema_version", "id", "released": bool }`.

## guard

```
ticketsplease guard <branch> [--base <ref>] [--ticket <id>] [--strict | --warn-collisions] [--explain] [--direct-only] [--ignore-transitive] [--config-ref <ref>] [--prefix tkt/]
```
Diffs the branch vs `--base` and makes two decoupled judgements, at **different severities**. An **under-declaration** (the branch touched a scope its ticket did not declare) is a **CONFLICT** — the file-authoritative, actionable signal — and fails with **exit 6**. A **declared-area overlap with an open sibling** is a **WARN** by default (reported, but **exit 0**): under parallel dispatch an overlap is the expected state, not a proven merge conflict, so it must not train agents to skim past the real signal. The verdict line is `CONFLICT | WARN | ok`. Requires a git repo (clean error otherwise).

It reads the `[scopes]` contract (and the `[guard]` section) from `--config-ref` (default: the base), **not** the possibly stale/empty config on the checked-out branch — so an emptied branch config can't give a false all-clear or silently downgrade the guard. Sibling tickets' in-flight status is read from `<prefix>*` branch tips, so a collision fires in the branch-per-ticket flow even when the current checkout shows the sibling as `todo`.

**Under-declaration is file-authoritative** (the cargo reverse-dep expansion never drives it; a `shared_scopes` claim counts as declared). **Collisions** use the full affected set (path globs + `[external_scopes]` pins + cargo reverse-deps), each tagged `cause`: `direct` (real overlap), `transitive` (reverse-dep only — safe for additive work), or `shared` (both tickets claim the scope additively — reported but **non-gating**, like `--ignore-transitive` for transitive). `warnings` flags scope-map gaps (changed files no scope covers) and an empty `[scopes]`.

**Make overlaps gate.** To restore hard-fail-on-overlap (the pre-`WARN`-default behaviour), pass `--strict` or set `[guard] gate_collisions = true` in `ticketsplease.toml` — the config is the default, the flags override per-invocation (`--strict` gates, `--warn-collisions` forces warn; the two are mutually exclusive). Under-declaration always gates regardless. When collisions gate, `--ignore-transitive` still waves through a transitive-only overlap.

`--explain` prints, under each affected / under-declared / colliding scope, the changed files that hit it (path-glob attribution; crate-graph and external-pin scopes are labelled by cause rather than per-file, and `unattributed` lists changed files no scope covers). The under-declaration note also spells out the exact `tkt set <id> --add-scope <scopes>` that fixes it.

Two ways to handle transitive noise, with different trade-offs (they matter when collisions gate — i.e. under `--strict` / `gate_collisions`):
- `--ignore-transitive` — **still computes and reports** transitive collisions (keeping `cause` visible for triage), but the gate ignores them: it fails only on a direct overlap or an under-declaration. `transitive_only` in the JSON is `true` when every gating collision is transitive. Use this for additive work where you want the report but not the block.
- `--direct-only` (alias `--no-reverse-deps`) — **skips the reverse-dep walk entirely**, so transitive collisions never appear in the report at all (faster, but no visibility). `[language] reverse_dep_expansion = false` makes that the repo default.

JSON (**schema_version 2**): `{ "schema_version", "ticket", "base", "branch", "changed_files", "affected_scopes", "affected_causes": { "<scope>": "direct"|"transitive" }, "declared_scopes", "under_declared", "collisions": [ {ticket, scopes, cause} ], "severity": "ok"|"warn"|"conflict", "conflict": bool, "transitive_only": bool, "warnings": [...], "explanation"?: { "scopes": { "<scope>": [files] }, "unattributed": [files] } }`. Read **`severity`** for the tier; `conflict` now means a *gating* conflict (`severity == "conflict"`) and excludes a warn-only overlap. `explanation` is present only with `--explain`.

## delete / rename

```
ticketsplease delete <id>
ticketsplease rename <old> <new>
```
`delete` removes the ticket file and its comments (git history preserves it). `rename` writes the new file, rewrites the `id`, repoints every dependent, moves the comments, then removes the old file (new-first, so an interruption never loses the ticket).

delete JSON: `{ "schema_version", "id", "deleted": true }`. rename JSON: `{ "schema_version", "old", "new", "repointed": [ids] }`.

## doctor / guide

```
ticketsplease doctor
ticketsplease guide
```
`doctor` validates setup: config present, git repo with a commit, scope globs compile, base ref resolves (exit non-zero on any failure). It also reports two **advisory** (non-gating) skill checks — `skill_canonical` (the canonical copy matches this binary; else run `skill sync`) and `skill_link` (the project links to it; else run `migrate`/`skill install`). JSON: `{ "schema_version", "ok": bool, "checks": [ {check, ok, detail} ] }`. `guide` prints the conceptual model (scopes, tracks, scoring, guard, claims). JSON: `{ "schema_version", "guide": "<text>" }`.

## states

```
ticketsplease states                       # list the effective workflow states + categories
ticketsplease migrate --remap old=new      # move tickets stranded in a renamed/removed state
```
By default a repo uses the built-in states (`todo`, `ready`, `in-progress`, `blocked`, `review`, `done`, `closed`). Define `[workflow.states]` in `ticketsplease.toml` to declare your own: each state's **name** is free, but it must pin to one engine **category** — `dispatchable` (pickable), `open` (occupies its scopes for the guard, blocks conflicting parallel work), `parked` (held, like `blocked`), or `terminal` (finished). A terminal state's `satisfies_dependents` bit *is* the done-vs-closed distinction (`true` unblocks dependents; `false` orphans them). The engine reasons on the category, never the name, so custom states schedule/guard/roll-up correctly and renaming a state (same category) never breaks anything. `set`/`create`/`reopen`/`watch --until` validate the status against this registry (an undefined state is exit 3). `states` JSON: `{ states: [{name, category, terminal, satisfies_dependents}], default, primary_open, primary_dropped, custom }`. When a config change removes/renames a state that live tickets still occupy, `lint` flags them `unknown-state` and `migrate --remap old=new` (repeatable) rewrites them.

**Enforced transitions (opt-in).** With `[workflow] enforce_transitions = true` and a `[workflow.transitions]` adjacency map (`from = [to, …]`), `set`/`close`/`reopen` reject any move that is not a listed edge (a `Conflict`, exit 6). Off by default (any-to-any) — the engine's invariants ride on categories, not edges, so add a graph only when you need a gate. Escape hatches: a `"*"` source (e.g. `"*" = ["closed"]` to close from anywhere) and `--force` on `set`/`close`/`reopen` (records `forced` on the event). `claim`/`release` are engine transitions, never gated. Bulk `set --where … --status X` advances the legal matches and reports each illegal one as `{id, changed: false, skipped: "illegal-transition"}` rather than aborting. `lint` flags `unknown-transition-state` (an edge naming an undefined state) and, under enforcement, `dead-end-nonterminal` (a non-terminal state with no way out).

## lint

```
ticketsplease lint
```
Validates schema (enums, id == filename, valid slug, duplicate ids, **unknown scope references** once a scope vocabulary exists, a scope claimed both exclusive and shared, a ticket that **declares `paths` but no scopes** — invisible to batching, see below, an **unknown workflow state**, resolution metadata on a non-closed ticket, and **workflow category coverage** — a config with no dispatchable or terminal state), links (dangling dependencies, dangling related links, and tickets **orphaned** by a closed dependency), and cycles — in one run, even when some files fail to parse. Exit 3 on schema/link problems, 5 on a cycle. Each finding carries a machine-readable `code` (`parse` | `id-mismatch` | `bad-id` | `unknown-scope` | `unknown-scope-policy` | `scope-mode-conflict` | `paths-without-scopes` | `duplicate-id` | `unknown-state` | `state-coverage` | `stale-resolution` | `missing-dep` | `missing-related` | `orphaned-by-closed-dep` | `cycle`). A dangling `related` is flagged but a `related` cycle is never an error.

`paths-without-scopes` catches a specific footgun: `paths` reads like a file-intent declaration, but only `guard` consumes it (as an under-declaration allowance). The scheduler — `tracks`, `why`, `lanes`, `next` — gates purely on **scope names**, so a ticket with `paths` and no `scopes`/`shared_scopes` is invisible to the conflict math and will be co-scheduled with work that rewrites the same files. The fix is to add a `scopes` entry (or `shared_scopes` for additive work). A scope-less **and** path-less ticket (a decision/epic/umbrella) declares no file intent and stays clean.

JSON: `{ "schema_version", "ok": bool, "diagnostics": [ {file, id, code, message} ] }`.

## skill install / sync / self-update

```
ticketsplease skill install [--harness claude|codex|opencode|pi-agent] [--global] [--dir <path>] [--copy]
ticketsplease skill sync
ticketsplease self-update [--version vX.Y.Z]
```
The skill content lives once at a canonical per-user path (`$XDG_DATA_HOME/ticketsplease/skill`, version-stamped); each install is a **symlink** to it, so refreshing the canonical copy updates every linked project at once. `skill install` creates that symlink for the chosen **`--harness`** — every harness reads the identical `SKILL.md` + `references/` layout, only the directory differs: `claude` → `.claude/skills`, `codex` → `.agents/skills` (the cross-tool Agent Skills standard dir, also read by opencode and Pi), `opencode` → `.opencode/skills`, `pi-agent` → `.pi/skills`. Project installs are gitignored (they point at an absolute path); `--global` installs into the harness's user-global dir instead (`~/.claude/skills`, `~/.agents/skills`, `~/.config/opencode/skills`, `~/.pi/agent/skills`), available in every project. `--dir <path>` overrides the project directory (not valid with `--global`); `--copy` writes a committable real copy instead of a symlink. `skill sync` re-extracts the canonical copy from the running binary — the installer runs it after install/`self-update`, so upgraders get the new skill automatically; `doctor` warns (non-gating) if the canonical copy or a project link has drifted, and `migrate` repairs a stale project link. `self-update` replaces the binary in place from GitHub Releases.

---

For orchestration patterns and merge-gate recovery (not flag syntax), see `references/parallel-workflow.md`.
