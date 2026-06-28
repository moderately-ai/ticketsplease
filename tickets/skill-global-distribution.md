---
id: skill-global-distribution
title: Per-project skill symlink to a canonical, installer-refreshed copy
status: todo
priority: p1
dependencies: []
related: []
scopes: [cli, skill, dist]
shared_scopes: []
paths: []
tags: [skill-dist, feature]
---
## Goal

A self-update (or install) automatically refreshes the skill in every project, with no stale per-project copies — by pointing each project at one canonical copy.

## Gap

skill::install copies the embedded skill into each repo's .claude/skills/ (a frozen snapshot). self-update only swaps the binary, so every project's copy goes stale and nothing detects it. Only the NEW binary can write the NEW skill, so the refresh must be driven by the new binary post-install.

## Work

- Canonical skill in the tool's own data dir: $XDG_DATA_HOME/ticketsplease/skill/ (default ~/.local/share/ticketsplease/skill/) + a version sentinel (env CARGO_PKG_VERSION). Not under ~/.claude — that is Claude-specific; the per-project link is the integration point.
- `tkt skill sync`: extract the embedded skill to the canonical dir (overwrite) + write the sentinel. Repo-agnostic (no project needed). Idempotent.
- install.sh: after installing the binary, run `ticketsplease skill sync` (best-effort) so install + self-update always refresh the canonical copy — and therefore every project symlinked to it.
- Per-project link: init / skill install create a SYMLINK <repo>/<base_dir>/ticketsplease -> canonical (base_dir defaults to .claude/skills, still overridable via --dir). Default = symlink; `--copy` keeps the old committed-real-copy behaviour as an opt-in. The symlink is local (absolute target), so add it to .gitignore on init rather than committing it.
- doctor / migrate: warn when the canonical sentinel != binary version (offer `skill sync`); detect a project path that is a stale real dir or a dangling / wrong-target link and repair it to a symlink -> canonical.
- Use std::os::unix::fs::symlink under cfg(unix); fall back to an extract-copy elsewhere. Resolve HOME/XDG via env.

## Acceptance

After `skill sync`, the canonical dir holds the current skill; `skill install` in a repo makes <repo>/.claude/skills/ticketsplease a symlink resolving to it; refreshing the canonical (a new `skill sync`) is seen by the linked project with no re-install. A simulated stale sentinel makes doctor flag drift; doctor repairs a real-dir project copy into a symlink. init commits no skill copy (the link is gitignored).

## Refs

Surfaced while shipping the v0.6 parallel-control skill content; must land in v0.6 so upgraders actually receive it. Per-project link (not user-global ~/.claude) so it works across different agent/config layouts.
