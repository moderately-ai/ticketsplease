---
id: doctor-duplicate-installs
title: "doctor: warn when ticketsplease and tkt resolve to different binaries or versions"
status: todo
priority: p3
dependencies: []
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [ux, onboarding, maint-advisory]
---
## Gap

The two documented install paths put binaries in different places: `cargo install` writes `~/.cargo/bin/ticketsplease`, while `install.sh` and `self-update` write `~/.local/bin/ticketsplease` plus a `tkt` symlink next to it. A user who has used both ends up with two copies, and `self-update` only refreshes the copy it runs from.

Seen on a real machine: `ticketsplease` resolved to `~/.cargo/bin` (v0.16, then v0.18 after `cargo install`), while `tkt` resolved to `~/.local/bin/ticketsplease` (v0.17). The two names silently ran different versions. Nothing reported it: `doctor` checks the skill copy's freshness but not the binaries, and `--version` only describes the copy that ran.

## Work

- In `doctor`, look up every `ticketsplease` and `tkt` on `PATH` (resolving symlinks). If they resolve to more than one distinct binary, or the binaries report different versions, add a non-gating warning listing each path and version, and the first `PATH` entry each name resolves to.
- The warning should name the fix: keep one install method, and remove or update the other copy.
- Add the same facts to `doctor --format json`.

## Acceptance

With two differing copies on `PATH`, `doctor` prints one non-gating warning listing both paths and versions and still exits 0. With a single install there is no new output. An integration test fakes `PATH` with two stub binaries.
