---
name: ticketsplease
description: Drive the ticketsplease CLI (`ticketsplease`, alias `tkt`) to manage git-native markdown tickets and dispatch conflict-free parallel work across multiple agents. Use this whenever you are coordinating work in a repo that has a `ticketsplease.toml` and a `tickets/` directory — deciding what to work on next, splitting work across several agents without merge collisions, creating/updating/linking tickets, or checking whether a branch's diff stayed inside its ticket's declared scope before merging. Reach for this skill whenever the user mentions tickets, parallel work, agent coordination, work distribution, "what should I work on", conflict-free batches, dependency-ordered work, or guarding/validating a branch — even if they do not name ticketsplease explicitly.
allowed-tools: Bash, Read, Write, Grep, Glob
---

# ticketsplease — git-native parallel-work ticketing

ticketsplease (CLI `ticketsplease`, short alias `tkt`) manages development work as **git-versioned markdown tickets** and computes **conflict-free parallel work assignment** — so work can be split across multiple agents that never edit the same area of the codebase at once. Tickets are plain markdown + YAML frontmatter under `tickets/`; **scopes** (abstract area names like `core` or `query/planner`) are defined in `ticketsplease.toml` and map to file globs — and, for Rust repos, to crates.

Use this skill to decide **what to work on**, to **split work safely across agents**, and to **guard a branch** before merging.

## Why this exists

When several agents work a repo in parallel, the failure mode is two of them editing the same files and colliding at merge time. ticketsplease prevents that two ways. `tracks` partitions ready work into batches where no two tickets share a scope, so an entire batch is safe to run in parallel. `guard` checks a branch's *actual* diff against its ticket's *declared* scope — failing if the branch wandered into an area it never claimed, or into one another open ticket owns. For Rust repos the guard maps the diff through the cargo crate graph, so a change to a leaf crate is also checked against everything that depends on it.

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
ticketsplease init        # scaffolds tickets/ + ticketsplease.toml (+ this skill)
ticketsplease guide       # the conceptual model in one screen (scopes, tracks, scoring, guard, claims)
ticketsplease doctor      # verify setup: config, git repo + commit, scope globs, base ref
```

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
   The claim is race-safe (a git-ref compare-and-swap: of N workers racing one ticket, exactly one wins and the rest get exit 6) and carries a lease, so a crashed worker's ticket becomes reclaimable instead of stuck forever. It also flips the ticket to in-progress, so `ready`/`tracks`/`next` stop offering it. It refuses a ticket whose dependencies aren't all done (exit 6), matching dispatch. On a clean claim, branch with the ticket id in the name (e.g. `tkt/<id>`) and work only inside the ticket's declared scope. This is what makes **pull-based** dispatch safe: many workers can each `claim` straight off the same `tracks`/`ready` pool with no central coordinator. To collapse the recommend-then-claim race into one call, use `ticketsplease next --claim --as <worker> --format json` — it atomically claims the best free pick (falling through to the next on a lost race). `ticketsplease claims` shows who holds what (assignee, lease, live/expired); `claim --force` steals a live lease.

3. **Before merging, guard the branch:**
   ```sh
   ticketsplease guard tkt/<id> --format json   # exit 6 → do not merge
   ```
   - Exit `0` → the diff stays within the declared scope and overlaps no open ticket's declared area; it clears this **pre-merge filter**. (This is a partitioning check, not a substitute for your normal build/test gate — disjoint branches can still conflict semantically.)
   - Exit `6` → a **declared-area overlap, not a proven conflict.** The JSON says where. `under_declared` is file-authoritative: scopes whose files the branch edited but that fall outside the ticket's declared area (declared-scope globs + `paths`). The crate-graph reverse-dep expansion never lands here — editing a foundational crate that many crates (or sibling sub-scopes) map to is not a scope escape, and a file named in `paths` is always covered. `collisions` lists open tickets whose declared area the affected set overlaps; each (and each scope in `affected_causes`) is tagged `direct` (real overlap) or `transitive` (reverse-dep only — usually safe for an additive change), and `guard --direct-only` drops the transitive ones. Resolve by narrowing the diff, declaring the scope (`ticketsplease set <id> --add-scope <scope>`), coordinating with the named ticket, or — if you own the merge — building+testing the combined result.

4. **Finish or release.** `claim` already set the ticket in-progress; on completion move it forward with `ticketsplease set <id> --status review|done` (setting `done` clears the claim). If you abandon the work, `ticketsplease release <id> --as <worker-id>` drops the claim and restores the pre-claim status (keeping any progress you'd advanced to). Renew a long-running claim by re-running `claim` (it extends your lease; a renewal logs no duplicate event).

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

- `ticketsplease ready` — dependency-satisfied tickets, priority-ordered (a ticket is ready when its status is todo/ready and every dependency is done).
- `ticketsplease tracks` — conflict-free parallel batches (the headline feature).
- `ticketsplease why <a> <b>` — explain whether two tickets can co-run, and if not, the exact reason (a shared scope, or one transitively depends on the other). Use it when the scheduler's grouping is surprising.
- `ticketsplease next [--parallel N] [--allow-overlap]` — scored recommendation(s); the score favours priority, critical-path position, and how much remaining downstream work the ticket unblocks. Picks are scope-disjoint by default; `--allow-overlap` returns the top-N even when scopes overlap, annotating each with the shared scopes so you can judge the file overlap yourself.
- `ticketsplease list [--status <s>]`, `ticketsplease show <id>`.

## Creating and editing tickets

```sh
ticketsplease create --title "Add vector index" --priority p1 \
  --scope query/planner --scope storage --depends-on build-index-trait
ticketsplease create --from backlog.json   # batch (JSON array; - reads stdin); validated all-or-nothing
ticketsplease set <id> --status in-progress --add-scope core --title "…" --add-path 'src/**' --add-dependency other
ticketsplease link <id> --depends-on <other-id>   # a cycle is rejected (exit 5); a dangling target is lint's job
ticketsplease rename <old> <new>            # moves the file, rewrites the id, repoints dependents
ticketsplease delete <id>                   # remove a ticket (git keeps history)
ticketsplease lint        # validate schema, scope refs, links, and cycles (exit 3 / 5 on problems)
```

Add `--dry-run` to `create`/`set` to preview without writing. Edits are **round-trip-safe**: ticketsplease rewrites only the field it changes and leaves everything else — custom frontmatter keys, comments, key order, and the markdown body — byte-for-byte. You can also hand-edit ticket files directly; they are just markdown.

Frontmatter schema: `id` (slug, equals the filename), `title`, `status` (todo/ready/in-progress/blocked/review/done), `priority` (p0 highest … p3), `dependencies[]` (ticket ids), `scopes[]` (names from `ticketsplease.toml`), `paths[]` (extra globs), `tags[]`. Claiming additionally manages `assignee` and `lease_expires_at` (epoch seconds) — leave those to `claim`/`release` rather than editing them by hand.

## Deeper references

- Full command/flag/JSON-shape reference: read `references/commands.md`.
- Multi-agent orchestration patterns (fan-out, branch naming, merge gating, recovering from a guard failure): read `references/parallel-workflow.md`.
