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

`init` / `create` / `set` / `link` / `show` / `ready` / `next` / `lint` are the convenience surface around those two; `status --all-branches` and `watch` give an orchestrator visibility into workers running on their own branches.

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
# edit ticketsplease.toml to define your scopes (see below)
tkt create --title "Add vector index" --priority p1 --scope query/planner
tkt create --id build-index-trait --title "Build the index trait" --scope core
tkt link add-vector-index --depends-on build-index-trait
tkt ready                             # what's dispatchable now
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
status: todo                  # todo | ready | in-progress | blocked | review | done
priority: p1                  # p0 (highest) .. p3
dependencies: [build-index-trait]
scopes: [query/planner]       # abstract area names defined in ticketsplease.toml
paths: []                     # extra explicit globs
tags: []
# any custom keys you add are preserved verbatim
---
Free-form markdown body.
```

Edits are **round-trip-safe**: ticketsplease rewrites only the field it changes and leaves unknown keys, key order, comments, and the body byte-for-byte. Hand-editing is fully supported.

A **scope** is a stable abstract name for an area of the codebase. Tickets reference scopes; `ticketsplease.toml` maps them to file globs (and, for Rust repos, to crates). Two tickets that share a scope never land in the same parallel batch, and the guard fails a branch that touches a scope its ticket didn't declare.

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
```

When `backend = "rust"`, the guard maps a branch's changed files to crates and walks the cargo **reverse-dependency** graph: a change to a leaf crate is flagged against every crate that depends on it. This needs `cargo` on `PATH` (always true inside a Rust repo). Each collision is tagged `cause: "direct"` (a real file/crate overlap) or `"transitive"` (reached only via the reverse-dep walk — safe for an additive change), and a per-scope `affected_causes` map lets a consumer triage which under-declarations and collisions are real rather than hand-diffing. Pass `guard --direct-only` (alias `--no-reverse-deps`) to gate on direct overlap only and skip the expansion entirely.

`[external_scopes]` extends the guard beyond this repo: a branch that bumps a pinned `git = … rev = …` dependency (matched by `repo` against the changed manifest lines) — or edits an in-tree fork `paths` glob — is flagged against tickets declaring that external scope. Because external scopes are ordinary scope names, `tracks` already keeps two tickets touching the same fork in separate batches.

## The contract

- **`--format json`** on every command yields a stable, versioned payload (`schema_version: 1`), deterministically ordered (sorted keys, no timestamps) — safe to diff and cache.
- **Exit codes are the API:** `0` ok · `2` usage · `3` invalid/dirty · `4` not found · `5` dependency cycle · `6` conflict · `7` watch timeout.
- Every command takes `--repo <path>`; everything is offline and atomic.

## The Claude skill

`tkt init` (and `tkt skill install`) drop a Claude skill into `.claude/skills/ticketsplease/`. It teaches an agent the orchestration loop — `tracks` to fan out disjoint work, `guard` to gate each branch before merge — and is embedded in the binary, so the installed copy always matches your version.

## Dogfooding

ticketsplease tracks its own development. See `tickets/` and `ticketsplease.toml` in this repo: `tkt tracks` reports which of the remaining work items can proceed in parallel.

## Status

v0.1, pre-release. The core is in place and tested: round-trip frontmatter editing, scheduling (`ready`/`tracks`/`next`), the conflict guard (path-glob + cargo crate-graph), and the bundled skill. Release binaries, self-update, and the migration engine are in progress (tracked as tickets in this repo).

## License

MIT — see [LICENSE](./LICENSE).
