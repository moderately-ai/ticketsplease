# ticketsplease

> git-native parallel-work ticketing — weave parallel work threads into one fabric

`ticketsplease` (short alias `tkt`) manages development work as **git-versioned markdown tickets** carrying dependency and affected-area metadata, and computes **conflict-free parallel work assignment** so multiple workers — primarily AI coding agents, humans secondarily — can be dispatched onto disjoint areas of a codebase without merge collisions. No server, no database: GitHub stays git-only.

> **Status: work in progress (v0.1, pre-release).** The scaffold and tooling are in place; commands are being implemented milestone by milestone.

## The two commands that matter

```sh
tkt tracks --format json        # conflict-free parallel batches of ready tickets
tkt guard <branch> --format json  # exit non-zero iff a branch's actual diff escapes
                                  # its ticket's declared scopes or overlaps another
```

`create` / `set` / `link` / `ready` / `next` / `init` / `lint` / `migrate` are the convenience surface around those two.

## Design tenets

- **CLI-first and fully scriptable.** Every command takes `--format json` emitting a stable, versioned schema. Human-readable is the default; JSON is the contract.
- **Exit codes are the API.** `0` ok · `3` invalid · `4` not found · `5` cycle · `6` conflict.
- **Deterministic.** Same inputs produce byte-identical output — no timestamps or randomness in machine output.
- **Offline and stateless.** Operates on an explicit `--repo` path and git ref; never assumes the network.
- **Round-trip-safe edits.** Editing a ticket does line-surgical frontmatter changes that preserve unknown keys, key order, and the body verbatim.

## Layout in a consuming repo

```
tickets/                 # browseable *.md tickets (ADR-style)
ticketsplease.toml       # scope → glob config, language backend, version pin
.claude/skills/ticketsplease/   # the bundled Claude skill (installed by `tkt init`)
```

## License

MIT — see [LICENSE](./LICENSE).
