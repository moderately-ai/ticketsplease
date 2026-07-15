---
id: update-check-advisory
title: "update-check advisory: probe latest release (no HTTP dep), cache, notify"
status: todo
priority: p1
dependencies: [advisory-output-channel, maintenance-config-table]
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [maint-advisory]
---
Silent staleness is real: a sweep of 6 repos found all 6 one release behind. Nudge — without breaking the tool's offline/minimal-dependency posture.

## Hard constraint

`update.rs` deliberately embeds **no HTTP/TLS stack** ("would dwarf the rest of the binary"; needs only `sh` + `curl`/`wget`). The update check must honour that — **do not add `reqwest`/`ureq`**.

## Proposed shape

- Probe `https://github.com/moderately-ai/ticketsplease/releases/latest` with `curl -fsSLI` (fallback `wget`), read the redirect's effective URL, parse the `vX.Y.Z` tag. This uses the redirect, not the GitHub API — so no rate limit and no auth.
- Compare to `CARGO_PKG_VERSION` with a tiny inline `X.Y.Z` parse (no `semver` dep).
- Cache `{ checked_at, latest }` at `$XDG_DATA_HOME/ticketsplease/update-check.json`; re-probe only when older than `check_interval_hours`. A timestamp in a state file is fine — it never touches the deterministic stdout contract.
- When newer **and** in advisory context **and** `[maintenance] update_check`, emit via the channel: `update available: 0.10.0 -> 0.11.0 — run `tkt self-update`` + the release URL. Show the delta so a behaviour-changing minor (e.g. 0.10.0's new lint) is not hidden.

## Decision

**Passive stderr notice** — the idiomatic Unix/POSIX behaviour (gh, rustup, npm's update-notifier, brew all notify passively; a blocking prompt breaks scripts and muscle memory). Never prompt, never block. Rate-limited by the cache. Honour `NO_COLOR`.

## Done when

Probe + compare + cache are unit-tested with an injectable clock and latest-tag; a fresh cache or `update_check = false` makes **no** network call; JSON / CI / non-TTY runs emit nothing and probe nothing; the notice shows the version delta and the self-update command.
