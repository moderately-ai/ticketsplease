---
id: lint-summary-advisory
title: lint summary in the health nudge (the signal doctor/migrate miss)
status: done
priority: p2
dependencies: [advisory-output-channel]
related: [drift-migrate-advisory]
scopes: [cli]
shared_scopes: []
paths: []
tags: [maint-advisory]
---
The actual hazard the 6-repo sweep found — 23 + 5 `paths-without-scopes` tickets — came from **`lint`**, which `doctor` does not run and `migrate` does not fix. If advisories exist to surface "things I should address", lint findings are the highest-value signal.

## Proposed shape

In advisory context, run lint once and, if findings > 0, emit a **count only** (not the list, never a gate): `board has N lint finding(s) — run `tkt lint``. Reuse the existing lint pass; a single run is cheap enough.

## Decision

**Included.** The health nudge surfaces both drift and a lint-findings count — this is the signal `doctor`/`migrate` miss and the one the 6-repo sweep actually turned up.

## Done when

The count shows when findings exist and is silent when clean; nothing in JSON / CI / non-TTY; folds into the same stderr block as the drift nudge without double-running lint.
