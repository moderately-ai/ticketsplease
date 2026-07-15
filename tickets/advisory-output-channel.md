---
id: advisory-output-channel
title: "advisory output channel: strictly-gated, stderr-only, agent-invisible"
status: todo
priority: p1
dependencies: []
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [maint-advisory, foundation]
---
The shared substrate every maintenance advisory rides on. Get the gating right here and the rest is detail.

## Proposed shape

A new `advisory` module in the cli crate plus a hook in `main.rs` after `cli::run` (both Ok and Err paths, before the `ExitCode` is returned).

- `is_advisory_context(fmt: Format) -> bool` — true **iff** all hold: stdout is a TTY **and** stdin is a TTY (`std::io::IsTerminal`, stable in our MSRV — no new dep); `fmt == Human`; the `CI` env var is unset; the opt-out `TICKETSPLEASE_NO_ADVISORIES` is unset.
- `emit(lines: &[String])` — write to **stderr only**, after the command's stdout has flushed.

## Non-negotiable constraints

This tool is agent-first: invoked in parallel, in CI, with `--format json` parsed downstream. Advisories must be **completely invisible** to non-interactive use — zero bytes on stdout, nothing in JSON mode, never a blocking prompt in the gated-out path, never a network stall on the hot path.

## Done when

The predicate is unit-tested for each gate (non-TTY, json, CI set, opt-out set each suppress); `main` emits nothing on a `--format json` or piped run; a smoke advisory appears only on an interactive human run; stdout is byte-identical with and without an advisory pending.
