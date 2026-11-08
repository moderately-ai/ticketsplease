---
id: skill-ignore-respects-git-excludes
title: "Skill link ignore: respect existing git excludes instead of always editing .gitignore"
status: done
priority: p2
dependencies: []
related: []
scopes: [cli, core, docs]
shared_scopes: []
paths: []
tags: [ux, bug, onboarding]
---
## Gap

`ensure_gitignored` (`crates/ticketsplease-cli/src/commands.rs:467`) decides whether the project skill link is ignored by reading only `<repo>/.gitignore`. It is called from `init` and `skill install` (`commands.rs:339`, `:413`) and from the auto-repair advisory (`advisory.rs:132`). A repo that already ignores the link another way, through `.git/info/exclude` or a global excludes file, gets the line appended to its tracked `.gitignore` anyway, so the tool dirties the working tree on every relink.

This hurts most in a fork that rebases onto an upstream: the tracked `.gitignore` belongs to upstream, so a local-only line there conflicts at each rebase. Found while setting up ticketsplease in a CLIProxyAPI fork (`local/maintained` rebased onto upstream `main`). The workaround, moving the entry to `.git/info/exclude`, is undone the next time `migrate` or the advisory relinks.

## Work

- Before writing, ask git whether the link path is already ignored (`git check-ignore -q <path>`, or the equivalent through the existing git helpers). If it is, write nothing.
- Let the user choose where a new entry goes: `.gitignore` (current default) or `.git/info/exclude`. For example an `init`/`skill install` flag plus a persisted config key, so `migrate` and the advisory follow the same choice.
- Outside a git repo, keep today's behaviour.

## Acceptance

- With the link path listed in `.git/info/exclude`, `init`, `skill install`, `migrate` and the auto-repair advisory leave `.gitignore` byte-identical.
- With the exclude-file option chosen, a fresh `init` writes the entry to `.git/info/exclude` and not to `.gitignore`.
- Tests cover both cases and the existing `.gitignore` default.
