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
ticketsplease show <id>
ticketsplease list [--status <s>]
```
`show --format human` prints the raw ticket file. `show --format json` → the ticket's fields. `list` → `{ "schema_version", "tickets": [ {id,title,status,priority} ] }`.

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
ticketsplease guard <branch> [--base <ref>] [--ticket <id>]
```
Computes the branch's diff vs `--base` (default `default_base` in config; three-dot merge-base diff), maps changed files to scopes (path globs, plus the cargo crate graph when `backend = "rust"`), and reconciles against the ticket's declared scopes. The ticket is taken from `--ticket`, else inferred from the branch name. **Exit 6** when the branch under-declares a scope or collides with another open ticket.

JSON: `{ "schema_version", "ticket", "base", "branch", "changed_files", "affected_scopes", "declared_scopes", "under_declared", "collisions": [ {ticket,scopes} ], "conflict": bool }`.

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
