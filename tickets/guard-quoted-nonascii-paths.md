---
id: guard-quoted-nonascii-paths
title: "guard: non-ASCII changed paths arrive quoted and match no scope"
status: todo
priority: p2
dependencies: []
related: [ux-guard-unscoped-paths, doctor-dead-scope-globs]
scopes: [core]
shared_scopes: []
paths: []
tags: [bug, guard]
---
## Gap

`diff_names` (`crates/ticketsplease-core/src/guard.rs:472`) runs `git diff --name-only` without `-z` and splits stdout on newlines. With git's default `core.quotePath=true`, any path containing non-ASCII bytes (or a quote, backslash or control character) comes back C-quoted, for example `"src/caf\303\251.rs"` instead of `src/café.rs`. The quoted string matches no scope glob, so guard treats a real edit as unscoped: an under-declaration on such a file is never reported, and the collision check misses it.

Reproduced on git 2.x: `git diff --name-only HEAD~1 HEAD` prints `"src/caf\303\251.rs"`; with `-z` it prints `src/café.rs`. Found while reviewing `doctor-dead-scope-globs`, whose new `tracked_files` already uses `ls-files -z`, so doctor and guard currently read the same file differently.

## Work

- Run `git diff --name-only -z` in `diff_names` and split on NUL, as `tracked_files` does.
- Check the other git path readers for the same issue (for example `store.rs`'s `ls-tree --name-only`). Ticket ids are sanitized slugs, so that one may be safe; confirm and leave a note.

## Acceptance

An integration test with a committed `src/café.rs` under a `src/**` scope shows guard attributing the change to that scope (under-declaration reported when the ticket doesn't declare it). Existing guard tests still pass.
