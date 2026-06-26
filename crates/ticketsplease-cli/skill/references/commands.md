# ticketsplease command reference

Global flags (accepted by every command):

- `--repo <path>` — repository root (default `.`).
- `--format human|json` — `human` is the default; `json` is the stable, versioned contract. Every JSON payload includes `"schema_version": 1` and is deterministically ordered.

Exit codes: `0` ok · `2` usage · `3` invalid/dirty · `4` not found · `5` cycle · `6` conflict.

## init

```
ticketsplease init [--dir tickets] [--force]
```
Scaffolds `<dir>/` and `ticketsplease.toml`, and installs the bundled skill into `.claude/skills/ticketsplease/`. Idempotent: an existing config is left untouched unless `--force`.

JSON: `{ "schema_version", "tickets_dir", "wrote_config" }`.

## create

```
ticketsplease create --title <s> [--id <slug>] [--status <s>] [--priority p0..p3]
                      [--depends-on a,b] [--scope x,y] [--path 'glob'] [--tag t] [--body <s>]
```
Writes a new ticket atomically. Without `--id`, the id is a slug of the title (with `-2`, `-3` … on collision). With `--id`, the create is **idempotent**: re-running with identical content is a no-op; different content with the same id is an error (exit 3).

JSON: `{ "schema_version", "id", "created": bool, "path" }`.

## set

```
ticketsplease set <id> [--status <s>] [--priority <p>]
                       [--add-scope a,b] [--remove-scope c] [--add-tag t]
```
Surgically updates fields (round-trip-safe). No-op if nothing changes.

JSON: `{ "schema_version", "id", "changed": bool }`.

## link

```
ticketsplease link <id> --depends-on <other> [--remove]
```
Adds (or with `--remove`, removes) a dependency edge. The target must exist (else exit 4); self-dependencies are rejected (exit 3).

JSON: `{ "schema_version", "id", "depends_on", "removed", "changed" }`.

## show / list

```
ticketsplease show <id> [--ref <branch>]
ticketsplease list [--status <s>]
```
`show --format human` prints the raw ticket file. `show --format json` → the ticket's fields. `--ref` reads the ticket as committed on a git ref (e.g. a `tkt/<id>` branch) instead of the working tree — no checkout needed. `list` → `{ "schema_version", "tickets": [ {id,title,status,priority} ] }`.

## status

```
ticketsplease status [--all-branches] [--prefix tkt/]
```
Without flags, the working-tree status of every ticket. `--all-branches` scans `refs/heads/<prefix>*` and reports each ticket's status as committed on its branch tip — so an orchestrator on `main` sees workers' in-flight status before merge (a branch whose ticket file is absent on its tip is reported with `status: null`). JSON: `{ "schema_version", "source": "worktree"|"branches", "tickets": [ {branch?, id, status, assignee, lease_expires_at} ] }`.

## watch

```
ticketsplease watch <id> --until <status> [--ref <branch>] [--prefix tkt/] [--interval 5] [--timeout <secs>]
```
Blocks, polling the ticket until it reaches `--until` (or `done`, which is always terminal), then exits 0. Without `--ref`, polls the `<prefix><id>` branch if it exists, else the working tree. **Exit 7** if `--timeout` seconds elapse first. The JSON payload is printed on both the success and timeout paths: `{ "schema_version", "id", "ref", "status", "reached": bool, "timed_out": bool }`.

## comment add / list

```
ticketsplease comment add <id> [--as <author>] [--reply-to <comment-id>] (--body <text> | --body-file <f|->)
ticketsplease comment list <id> [--ref <branch>]
```
`comment add` appends a comment as its own file under `<tickets_dir>/<id>.comments/<comment-id>.md` (one file per comment, so concurrent authors never conflict — no lock, no merge driver, in both shared-worktree and single-clone topologies). `--body-file -` reads stdin (shell-safe for rich markdown). The ticket must exist (else exit 4). `comment list` returns comments sorted chronologically; `--ref` reads them as committed on a branch (so an orchestrator on `main` sees a worker's comments). `tkt show <id>` also folds comments in (human: a `## Comments` section; JSON: a `comments` array). JSON: `{ "schema_version", "ticket", "comments": [ {id, by, at, reply_to, body} ] }`.

Adding a comment also emits an **event** (below), so a watcher is notified live.

## events

```
ticketsplease events [--since <event-id>] [--ticket <id>] [--type <kind>]
```
The cross-branch activity log: each event is a `refs/ticketsplease/events/<id>` ref pointing at a JSON blob, living entirely in `.git`. So events are visible across worktrees and a shared clone **immediately — no commit, no push, no merge** — and concurrent emits never collide (per-ref atomic create). The id is time-sortable; pass `--since <last-seen-id>` as a cursor for resumable tailing that never misses a transition. `--ticket` / `--type` filter. JSON: `{ "schema_version", "events": [ {id, ticket, kind, by, at, data} ] }`. (Empty when there's no git repo — the event log is the live signal; the comment files are the durable record.)

## ready

```
ticketsplease ready
```
Dispatchable tickets (status todo/ready with every dependency done), ordered by `(priority, id)`. A dependency cycle is a hard error (exit 5).

JSON: `{ "schema_version", "ready": [ {id,title,status,priority,scopes} ] }`.

## tracks

```
ticketsplease tracks
```
Partitions the ready set into conflict-free batches: no two tickets in a batch share a scope or sit in the same dependency component. Dispatch one batch fully in parallel.

JSON: `{ "schema_version", "batches": [ [ {id,title,status,priority,scopes} ] ] }`.

## next

```
ticketsplease next [--parallel N]
```
The single highest-scored dispatchable ticket, or N mutually conflict-free picks. Score favours priority, downstream critical-path length, and transitive unblock count.

JSON: `{ "schema_version", "picks": [ {id,title,status,priority,scopes,score} ] }`.

## claim / release

```
ticketsplease claim <id> --as <worker> [--ttl <secs>]      # default ttl 3600
ticketsplease release <id> [--as <worker>] [--force]
```
`claim` atomically takes a ticket for a worker and marks it in-progress. Atomicity is a git-ref compare-and-swap (`refs/ticketsplease/claim/<id>` created with `git update-ref`'s create-only mode): of N workers racing one ticket, exactly one wins and the rest get **exit 6**. The claim records `assignee` + `lease_expires_at` in the frontmatter; once the lease expires the ticket is reclaimable, so a crashed worker does not strand it (the next claimer takes over with `"stolen": true`). Re-claiming as the same worker extends the lease. Only todo/ready/in-progress tickets are claimable (else exit 3). The lock lives in `.git`, coordinating across worktrees and a single checkout offline.

`release` drops the claim and returns the ticket to `ready`. Without `--force`, only the recorded holder may release (a non-holder gets exit 6); releasing an unclaimed ticket is a no-op success.

claim JSON: `{ "schema_version", "id", "assignee", "lease_expires_at", "stolen": bool }`.
release JSON: `{ "schema_version", "id", "released": bool }`.

## guard

```
ticketsplease guard <branch> [--base <ref>] [--ticket <id>] [--direct-only]
```
Computes the branch's diff vs `--base` (default `default_base` in config; three-dot merge-base diff), maps changed files to scopes (path globs, plus the cargo crate graph when `backend = "rust"`, plus `[external_scopes]` pins), and reconciles against the ticket's declared scopes. The ticket is taken from `--ticket`, else inferred from the branch name. **Exit 6** when the branch under-declares a scope or collides with another open ticket.

Each collision carries a `cause`: `direct` (a real file/crate overlap) or `transitive` (reached only via the cargo reverse-dependency walk — safe for an additive change). The `affected_causes` map tags every affected scope the same way, so you can auto-triage exit 6 (e.g. ignore purely-`transitive` collisions for additive work) instead of hand-diffing. `--direct-only` (alias `--no-reverse-deps`) skips the reverse-dep expansion and gates on direct overlap only — clearing both transitive collisions and transitive under-declarations. `[external_scopes]` additionally flags a bumped `git = … rev = …` pin (matched by `repo`) or an in-tree fork path.

JSON: `{ "schema_version", "ticket", "base", "branch", "changed_files", "affected_scopes", "affected_causes": { "<scope>": "direct"|"transitive" }, "declared_scopes", "under_declared", "collisions": [ {ticket, scopes, cause} ], "conflict": bool }`.

## lint

```
ticketsplease lint
```
Validates schema (enums, id == filename, duplicate ids), links (dangling dependencies), and cycles. Exit 3 on schema/link problems, 5 on a cycle.

JSON: `{ "schema_version", "ok": bool, "diagnostics": [ {file,id,message} ] }`.

## skill install / self-update

```
ticketsplease skill install [--dir .claude/skills]
ticketsplease self-update [--version vX.Y.Z]
```
`skill install` writes the bundled skill (the version baked into the running binary). `self-update` replaces the binary in place from GitHub Releases.
