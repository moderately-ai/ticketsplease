# ticketsplease

> git-native parallel-work ticketing — weave parallel work threads into one fabric

`ticketsplease` (short alias `tkt`) manages development work as **git-versioned markdown tickets** carrying dependency and affected-area metadata, and computes **conflict-free parallel work assignment** so multiple workers — primarily AI coding agents, humans secondarily — can be dispatched onto disjoint areas of a codebase without merge collisions. No server, no database: GitHub stays git-only.

It's driven from the command line and built to be scripted: every command speaks JSON, exit codes are the API, and output is deterministic.

## The two commands that matter

```sh
tkt tracks --format json          # conflict-free parallel batches of ready tickets
tkt guard <branch> --format json  # exit 6 iff a branch's actual diff escapes its
                                  # ticket's declared scope or collides with another
```

`init` / `create` / `set` / `link` / `show` / `list` / `ready` / `next` / `lint` / `delete` / `rename` are the convenience surface around those two; `list --where` filters with boolean expressions and `view` saves them as named views; `rollup` aggregates an initiative (counts, % done, ready frontier, blocked set) and `graph` / `path` export the dependency DAG (Graphviz DOT, critical path); `tracks --max-overlap` / `lanes` tune how parallel you go (tolerate benign overlap, or sequence conflicts onto per-worker lanes instead of idling); `claim` / `release` / `claims` / `next --claim` provide race-safe pull-based dispatch; `status --all-branches`, `reconcile`, `watch`, `comment`, and `events --watch` give an orchestrator a live view of — and an append-only, conflict-free annotation channel for — workers running on their own branches (`reconcile` flags where the board has drifted from the actual branches/worktrees). New to the model? `tkt guide` prints it in one screen, and `tkt doctor` verifies setup.

## Install

**From source (works today):**

```sh
cargo install --git https://github.com/moderately-ai/ticketsplease --locked ticketsplease-cli
```

This installs the `ticketsplease` binary. Symlink a short alias if you like: `ln -s "$(command -v ticketsplease)" ~/.local/bin/tkt`.

**Prebuilt binary via the installer** (once a release is tagged):

```sh
curl -fsSL https://raw.githubusercontent.com/moderately-ai/ticketsplease/main/install.sh | sh
```

The installer detects your platform, downloads the matching static binary, verifies its SHA256, installs to `~/.local/bin`, and symlinks `tkt`. Pin a version with `TICKETSPLEASE_VERSION=v0.1.0`, or change the directory with `BIN_DIR=…`.

## Quickstart

```sh
tkt init                              # scaffold tickets/ + ticketsplease.toml + the Claude skill
tkt guide                             # the conceptual model in one screen
# edit ticketsplease.toml to define your scopes (see below)
tkt create --title "Add vector index" --priority p1 --scope query/planner
tkt create --id build-index-trait --title "Build the index trait" --scope core
tkt link add-vector-index --depends-on build-index-trait
tkt ready                             # what's dispatchable now
tkt list --where 'priority:p0 AND NOT status:done'   # boolean filter (AND/OR/NOT, parens)
tkt view save epic 'tag:epic AND NOT status:done'    # save a reusable named view, then: tkt list --view epic
tkt tracks                            # conflict-free parallel batches
tkt next --parallel 4                 # four disjoint picks for four agents
tkt guard my-branch                   # gate a branch before merge (exit 6 = conflict)
tkt status --all-branches             # each worker's tip status across tkt/* branches
tkt watch add-vector-index --until review --timeout 600  # block until a worker is ready (exit 7 on timeout)
tkt lint                              # validate schema, links, and cycles
```

## The model

A **ticket** is a markdown file under `tickets/` with YAML frontmatter:

```yaml
---
id: add-vector-index          # slug; equals the filename
title: Add vector index
status: todo                  # built-in: todo|ready|in-progress|blocked|review|done|closed (or your [workflow.states])
priority: p1                  # p0 (highest) .. p3
dependencies: [build-index-trait]   # hard, scheduling-blocking; cycle-checked
related: []                   # soft "see also"; ignored by scheduling
scopes: [query/planner]       # exclusive (rewrite) area claims
shared_scopes: []             # additive (append) claims — co-edit freely
paths: []                     # extra explicit globs
tags: []
# any custom keys you add are preserved verbatim
---
Free-form markdown body.
```

Edits are **round-trip-safe**: ticketsplease rewrites only the field it changes and leaves unknown keys, key order, comments, and the body byte-for-byte. Hand-editing is fully supported.

**Terminal states — `done` vs `closed`.** Both take a ticket out of scheduling, but they mean different things. `done` = completed: it *satisfies* dependents, so they become ready. `closed` = terminated without completing (won't-do, duplicate, obsolete, superseded, cancelled): it does **not** satisfy dependents — instead they surface as **orphaned** (`rollup` lists them, `lint` fails on them, and `claim` refuses with a pointed message) so you re-point, waive, or close them rather than silently building on abandoned work. `tkt close <id> --reason <duplicate|wontdo|obsolete|superseded|cancelled> --note <text>` records an optional resolution; `tkt reopen <id>` returns it to an active status and clears the reason in the same write. The reason is queryable (`list --where 'reason:duplicate'`).

A **scope** is a stable abstract name for an area of the codebase. Tickets reference scopes; `ticketsplease.toml` maps them to file globs (and, for Rust repos, to crates). Two tickets that share a scope never land in the same parallel batch, and the guard fails a branch that touches a scope its ticket didn't declare.

**Access intent.** A scope can be claimed *exclusively* (`scopes` — a rewrite) or *shared/additively* (`shared_scopes` — append/extend). Two shared claims on a scope are compatible and run in parallel; a shared claim still conflicts with an exclusive one. On top of that, `tracks`/`next`/`lanes` take `--max-overlap K`, a per-pair tolerance budget (`0` strict … `any`), so you fill N workers least-riskily instead of single-threading on benign clashes — `tracks --width` tells you how many fit, and `lanes` plans ordered per-worker queues that *sequence* conflicts instead of dropping them. `[scope_policy]` weights a scope's clash cost (`0` = free hub). The guard honours all of this: a shared-by-both collision is reported but non-gating.

## Configuration — `ticketsplease.toml`

```toml
schema_version = 1
tickets_dir = "tickets"
default_base = "main"          # base ref for `guard`

[language]
backend = "rust"               # "none" (path globs only) or "rust" (also use the cargo crate graph)

[scopes]                       # scope name -> path globs
"query/planner" = ["crates/query/src/planner/**"]
"core"          = ["crates/core/**"]

[scope_crates]                 # scope -> owning crate, so the guard expands reverse-dependents
"core" = "my-core-crate"

[external_scopes]              # name a forked dep (pinned via git=…rev=…) as a scope
"sqlparser-fork" = { repo = "tomsanbear/sqlparser", paths = [] }

[scope_policy]                 # per-scope clash cost for tracks/next --max-overlap
"core" = { weight = 0 }        # weight 0 = a free-to-co-edit hub; higher = riskier (default 1)

[workflow]                     # custom lifecycle states (optional; omit for the built-in set)
[workflow.states.todo]
category = "dispatchable"       # dispatchable | open | parked | terminal — the engine contract
[workflow.states.qa]
category = "open"              # occupies its scopes for the guard; excluded from `ready`
[workflow.states.shipped]
category = "terminal"
satisfies_dependents = true    # a completed terminal state — unblocks dependents
[workflow.states.wontfix]
category = "terminal"
satisfies_dependents = false   # a dropped/cancelled terminal state — orphans dependents
```

When `backend = "rust"`, the guard maps a branch's changed files to crates and walks the cargo **reverse-dependency** graph: a change to a leaf crate is flagged against every crate that depends on it. This needs `cargo` on `PATH` (always true inside a Rust repo). Each collision is tagged `cause: "direct"` (a real file/crate overlap) or `"transitive"` (reached only via the reverse-dep walk — safe for an additive change), and a per-scope `affected_causes` map lets a consumer triage which under-declarations and collisions are real rather than hand-diffing. Pass `guard --direct-only` (alias `--no-reverse-deps`) to gate on direct overlap only and skip the expansion entirely.

`[external_scopes]` extends the guard beyond this repo: a branch that bumps a pinned `git = … rev = …` dependency (matched by `repo` against the changed manifest lines) — or edits an in-tree fork `paths` glob — is flagged against tickets declaring that external scope. Because external scopes are ordinary scope names, `tracks` already keeps two tickets touching the same fork in separate batches.

**Custom workflow states.** With no `[workflow]` table a repo uses the built-in states (`todo`, `ready`, `in-progress`, `blocked`, `review`, `done`, `closed`). Define `[workflow.states]` to declare your own — each state's **name** is yours to choose, but it must be pinned to one engine **category** the scheduler/guard/rollup reason about: `dispatchable` (pickable), `open` (occupies its scopes for the guard, blocks conflicting parallel work), `parked` (held but not finished, like `blocked`), or `terminal` (finished, excluded from scheduling). A terminal state's `satisfies_dependents` bit *is* the done-vs-closed distinction — `true` unblocks dependents, `false` orphans them. `tkt states` lists the effective registry; `tkt lint` rejects a config with no dispatchable or terminal state; `tkt migrate --remap old=new` moves tickets stranded by a renamed/removed state. Because the engine keys on the category (never the name), renaming a state while keeping its category never breaks scheduling.

## Tool-managed state — `.ticketsplease/`

Saved views (and bundled body templates) live under `.ticketsplease/` at the repo root. Unlike most tool dot-dirs this is **meant to be committed** — a saved view like "the open p0/p1 epic" or a shared ticket-body template is a project artifact, not local state. Don't add it to `.gitignore`.

## The contract

- **`--format json`** on every command yields a stable, versioned payload (`schema_version: 1`), deterministically ordered (sorted keys, no timestamps) — safe to diff and cache.
- **Exit codes are the API:** `0` ok · `2` usage · `3` invalid/dirty · `4` not found · `5` dependency cycle · `6` conflict · `7` watch timeout.
- Every command takes `--repo <path>`; everything is offline and atomic.

## The Claude skill

`tkt init` (and `tkt skill install`) wire a Claude skill into `.claude/skills/ticketsplease/`. It teaches an agent the orchestration loop — `tracks` to fan out disjoint work, `guard` to gate each branch before merge.

The skill is embedded in the binary, but instead of a frozen per-repo copy it lives once at a canonical per-user path (`~/.local/share/ticketsplease/skill`) and each project's `.claude/skills/ticketsplease` is a **symlink** to it. The installer runs `tkt skill sync` after every install/`self-update`, so the canonical copy — and therefore every linked project — always matches your binary; `tkt doctor` warns if it drifts and `tkt migrate` repairs a stale link. The link is local, so `init` gitignores it; use `tkt skill install --copy` if you'd rather commit a real copy.

## Dogfooding

ticketsplease tracks its own development. See `tickets/` and `ticketsplease.toml` in this repo: `tkt tracks` reports which of the remaining work items can proceed in parallel.

## Status

v0.1, pre-release. The core is in place and tested: round-trip frontmatter editing, scheduling (`ready`/`tracks`/`next`), the conflict guard (path-glob + cargo crate-graph), and the bundled skill. Release binaries, self-update, and the migration engine are in progress (tracked as tickets in this repo).

## License

MIT — see [LICENSE](./LICENSE).
