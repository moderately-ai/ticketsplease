# ticketsplease command reference

Global flags (accepted by every command):

- `--repo <path>` — repository root (default `.`).
- `--format human|json` — `human` is the default; `json` is the stable, versioned contract. Every JSON payload includes `"schema_version": 1` and is deterministically ordered.

Exit codes: `0` ok · `2` usage · `3` invalid/dirty · `4` not found · `5` cycle · `6` conflict · `7` timeout (`watch` / `events --watch`).

## JSON conventions

- **Result key per command.** Each command's payload carries its result under a stable, documented key, listed with the command below. The quick map: `init`→(fields) · `create`→`results` · `set`→(fields) · `link`→(fields) · `show`→(fields) · `list`→`tickets` · `status`→`tickets` · `reconcile`→`findings` · `claims`→`claims` · `ready`→`ready` · `tracks`→`batches` · `next`→`picks` (or `claimed` with `--claim`) · `why`→(fields) · `guard`→(fields) · `lint`→`diagnostics` · `comment list`→`comments` · `events`→`events` · `doctor`→`checks` · `guide`→`guide` · `delete`/`rename`→(fields).
- **`id` vs `ticket`.** When an object *is* a ticket (show/list/ready/status/claims), its id is `id`. When an object *references* a ticket from elsewhere (a comment, an event, a collision, a `conflicts_with` entry), the referenced ticket is `ticket` and the object's own id (if any) is `id`/`comment_id`. So `id` is always "this object", `ticket` is always "the ticket it's about".
- **`depends_on` in, `dependencies` out.** Inputs that accept dependencies use `depends_on` (`create --depends-on`, `link --depends-on`, and the batch spec key, which also accepts `dependencies` as an alias). Stored/queried output always uses `dependencies`.
- **`dependencies` block; `related` does not.** `dependencies` gate scheduling (a ticket is not `ready` until all are `done`) and are cycle-checked. `related` is a soft, non-blocking cross-reference: recorded, queryable (`--where related:x`), and graphable, but ignored by `ready`/`tracks`/`next`/cycle-detection. Use `related` for "see also", `depends_on` for "must finish first".

## init

```
ticketsplease init [--dir tickets] [--force]
```
Scaffolds `<dir>/` and `ticketsplease.toml`, and installs the bundled skill into `.claude/skills/ticketsplease/`. Idempotent: an existing config is left untouched unless `--force`. Prints a next-steps block, and warns if the directory is not a git repo (claim/guard/status/events/watch need `git init` + a commit).

JSON: `{ "schema_version", "tickets_dir", "wrote_config", "skill_installed", "git": bool }`.

## create

```
ticketsplease create --title <s> [--id <slug>] [--status <s>] [--priority p0..p3]
                      [--depends-on a,b] [--related c,d] [--scope x,y] [--path 'glob'] [--tag t]
                      [--body <s>] [--dry-run]
ticketsplease create --from <file|-> [--dry-run]
```
Writes new tickets atomically. Without `--id`, the id is a slug of the title and the create is **content-addressed-idempotent**: re-running the same create is a no-op (`created: false`), not a `<slug>-2` clone; a genuinely different ticket at that slug takes the next suffix. With `--id`, re-running with identical content is a no-op; different content with the same id is an error (exit 3).

`--from` batch-creates from a **JSON array** of specs or a **TOML `[[ticket]]`** document (format chosen by `.json`/`.toml` extension; `-` reads stdin, defaulting to JSON unless the content starts with `[[`). Each spec is `{title, id?, status?, priority?, depends_on?, related?, scopes?, paths?, tags?, body?}`. Unknown keys are **rejected** (a typo like `dependson` fails loudly). The whole batch is validated before any write (a bad element aborts before partial state). `--dry-run` previews without writing.

JSON (single and batch share one shape): `{ "schema_version", "results": [ {id, created: bool, path} ], "dry_run": bool }`.

## set

```
ticketsplease set (<id> | --where <expr> | --view <name>)
                       [--title <s>] [--status <s>] [--priority <p>]
                       [--add-scope a,b] [--remove-scope c] [--add-tag t] [--remove-tag u]
                       [--add-path 'glob'] [--remove-path 'glob']
                       [--add-dependency d] [--remove-dependency e]
                       [--add-related r] [--remove-related s]
                       [--body <s> | --body-file <f|-> | --append-body <s> | --append-body-file <f|->] [--dry-run]
```
Surgically updates fields (round-trip-safe), writing back to the file it read even if the frontmatter `id` has drifted from the filename. No-op if nothing changes. `--add-dependency` is rejected if it would close a cycle (exit 5), like `link`; `--add-related` is never cycle-checked. Setting status `done` clears the claim (assignee + lease). `--dry-run` previews without writing.

**Single vs bulk:** pass an `id` to edit one ticket, or `--where`/`--view` to edit **every matching ticket** in one operation (exactly one of the two; passing both, or neither, is exit 3). Bulk applies field edits only — `--title` and the body edits are single-target and rejected with `--where`/`--view`. A single cycle check runs over the whole edited set after all dependency edits.

Single JSON: `{ "schema_version", "id", "changed": bool, "dry_run": bool }`.
Bulk JSON: `{ "schema_version", "matched": N, "results": [ {id, changed: bool} ], "dry_run": bool }`.

## link

```
ticketsplease link <id> (--depends-on <other> | --related <other>) [--remove]
```
Adds (or with `--remove`, removes) a link. `--depends-on` is a hard, cycle-checked **dependency** edge; `--related` is a soft, non-blocking cross-reference that scheduling ignores (and so is never cycle-checked). Exactly one of the two is required. A dangling target is **permitted** (lint reports it as `missing-dep`/`missing-related`) — consistent with `create`; only a dependency edge that closes a **cycle** is rejected at write time (exit 5). `--remove` never validates the target, so a link to a deleted ticket can be cleaned. A self-link is rejected (exit 3).

JSON: `{ "schema_version", "id", "depends_on"|"related", "removed", "changed" }`.

## show / list

```
ticketsplease show <id> [--ref <branch>]
ticketsplease list [--status <s>] [--scope <s>] [--tag <t>] [--priority <p>] [--where <expr>] [--view <name>] [--hide-done]
```
`show --format human` prints a rendered field view + body + comments; `--format json` → the ticket's fields. `--ref` reads the ticket as committed on a git ref (no checkout). `list` filters compose (AND); `--hide-done` drops completed tickets. A malformed ticket file degrades to a warning rather than failing the listing.

`--where` is a boolean filter expression: `field:value` terms joined by `AND` / `OR` / `NOT` (case-insensitive) with parentheses; it composes (AND) with the single-axis flags. Fields: `status`, `priority`, `tag`, `scope`, `assignee`, `id`, `dep`, `related`. Values are barewords (`p0`, `query/planner`, slug ids) or quoted (`"needs review"`). `status:`/`priority:` values are validated, so a typo exits 3. Examples: `--where 'tag:dialect AND NOT status:done'`, `--where '(priority:p0 OR priority:p1) AND scope:core'`. `--view <name>` applies a saved expression and ANDs with `--where`.

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

JSON: `{ "schema_version", "selector": {tag,where,view}, "total", "done", "percent_done", "by_status": {status: n}, "by_priority": {p: n}, "ready": [ {id,title,priority} ], "blocked": [ {id,title,unmet: [ids]} ] }`.

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
- `in-progress-no-branch` — a ticket marked in-progress with no work branch (abandoned or never-started dispatch; **stale-busy**).
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
ticketsplease tracks [--parallel N]
```
Partitions the ready set into conflict-free batches: no two tickets in a batch share a scope. Dispatch one batch fully in parallel. `--parallel N` caps each batch to N tickets (splitting larger ones), giving an orchestrator worker-sized fronts.

JSON: `{ "schema_version", "batches": [ [ {id,title,status,priority,scopes,...} ] ] }`.

## next

```
ticketsplease next [--parallel N] [--allow-overlap] [--claim --as <worker> [--ttl <secs>]]
```
The highest-scored dispatchable ticket(s). **Score** = `1000 × priority (p0=3..p3=0) + 10 × critical-path length + count of not-done tickets it unblocks` — higher is more impactful. Picks are scope-disjoint by default; `--allow-overlap` returns the top-N by score even when scopes overlap, annotating each with `conflicts_with`. `--claim --as <worker>` atomically claims the first still-free pick (race-safe dispatch in one call; a lost race falls through to the next pick).

JSON: `{ "schema_version", "picks": [ {id,...,score, "conflicts_with": [ {ticket,scopes} ]} ] }`, or with `--claim`: a claim payload (see below) or `{ "schema_version", "claimed": null }` when nothing is free.

## why

```
ticketsplease why <a> <b>
```
Explains whether two *different* tickets can run in parallel (passing the same id twice is a usage error, exit 3). They cannot if they share a scope **or** one transitively depends on the other. JSON: `{ "schema_version", "a", "b", "conflict": bool, "shared_scopes": [...], "dependency_ordered": bool }`. Exits 6 on conflict (so `why a b && …` gates).

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
ticketsplease guard <branch> [--base <ref>] [--ticket <id>] [--direct-only] [--ignore-transitive] [--config-ref <ref>] [--prefix tkt/]
```
Diffs the branch vs `--base` and makes two decoupled judgements. **Exit 6** when the branch under-declares a scope or collides with another open ticket. Requires a git repo (clean error otherwise).

It reads the `[scopes]` contract from `--config-ref` (default: the base), **not** the possibly stale/empty config on the checked-out branch — so an emptied branch config can't give a false all-clear. Sibling tickets' in-flight status is read from `<prefix>*` branch tips, so a collision fires in the branch-per-ticket flow even when the current checkout shows the sibling as `todo`.

**Under-declaration is file-authoritative** (the cargo reverse-dep expansion never drives it). **Collisions** use the full affected set (path globs + `[external_scopes]` pins + cargo reverse-deps), each tagged `cause`: `direct` (real overlap) or `transitive` (reverse-dep only — safe for additive work). `warnings` flags scope-map gaps (changed files no scope covers) and an empty `[scopes]`.

Two ways to handle transitive noise, with different trade-offs:
- `--ignore-transitive` — **still computes and reports** transitive collisions (keeping `cause` visible for triage), but the *exit code* ignores them: the gate fails only on a direct overlap or an under-declaration. `transitive_only` in the JSON is `true` when a conflict exists but every part of it is transitive (so a gate would otherwise have been a false 6). Use this for additive work where you want the report but not the block.
- `--direct-only` (alias `--no-reverse-deps`) — **skips the reverse-dep walk entirely**, so transitive collisions never appear in the report at all (faster, but no visibility). `[language] reverse_dep_expansion = false` makes that the repo default.

JSON: `{ "schema_version", "ticket", "base", "branch", "changed_files", "affected_scopes", "affected_causes": { "<scope>": "direct"|"transitive" }, "declared_scopes", "under_declared", "collisions": [ {ticket, scopes, cause} ], "conflict": bool, "transitive_only": bool, "warnings": [...] }`. (`conflict` stays strict — any conflict; gate on the exit code, or on `transitive_only` to auto-allow transitive-only.)

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
`doctor` validates setup: config present, git repo with a commit, scope globs compile, base ref resolves (exit non-zero on any failure). JSON: `{ "schema_version", "ok": bool, "checks": [ {check, ok, detail} ] }`. `guide` prints the conceptual model (scopes, tracks, scoring, guard, claims). JSON: `{ "schema_version", "guide": "<text>" }`.

## lint

```
ticketsplease lint
```
Validates schema (enums, id == filename, valid slug, duplicate ids, **unknown scope references** once a scope vocabulary exists), links (dangling dependencies and dangling related links), and cycles — in one run, even when some files fail to parse. Exit 3 on schema/link problems, 5 on a cycle. Each finding carries a machine-readable `code` (`parse` | `id-mismatch` | `bad-id` | `unknown-scope` | `duplicate-id` | `missing-dep` | `missing-related` | `cycle`). A dangling `related` is flagged but a `related` cycle is never an error.

JSON: `{ "schema_version", "ok": bool, "diagnostics": [ {file, id, code, message} ] }`.

## skill install / self-update

```
ticketsplease skill install [--dir .claude/skills]
ticketsplease self-update [--version vX.Y.Z]
```
`skill install` writes the bundled skill (the version baked into the running binary). `self-update` replaces the binary in place from GitHub Releases.
