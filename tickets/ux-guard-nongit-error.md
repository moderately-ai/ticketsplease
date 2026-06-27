---
id: ux-guard-nongit-error
title: guard in a non-git dir dumps full git-diff usage
status: done
priority: p1
dependencies: []
scopes: [core]
paths: []
tags: [ux, bug]
---
guard shells out to `git diff main...branch`; in a non-git dir git falls back to --no-index and prints ~100 lines of usage, burying the real error.
Fix: pre-check for a git repo (like claim does) and emit a clean message: 'this command requires a git repository (git init + at least one commit)'.
Found by: onboarding agent.
