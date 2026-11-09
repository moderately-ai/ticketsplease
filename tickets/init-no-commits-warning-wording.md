---
id: init-no-commits-warning-wording
title: "init: a repo with no commits is reported as 'not a git repository'"
status: todo
priority: p3
dependencies: []
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [ux, onboarding, bug]
---
## Gap

`tkt init` in a freshly cloned empty repository (a `.git` directory on branch `main` with no commits, as `gh repo create --clone` produces) prints:

    warning: not a git repository — claim/guard/status/events/watch need `git init` and at least one commit.

The directory is a git repository; it only lacks a commit. The advice to run `git init` is wrong there and would be harmless but confusing. Seen on 2026-11-09 setting up `tomsanbear/cliproxyapi-archiver`.

## Work

Distinguish the two cases in the check `init` runs: outside a work tree → the current message; inside a work tree with no `HEAD` commit → "git repository has no commits yet — claim/guard/status/events/watch need at least one commit". `doctor`'s `git_repo` check already phrases it as "git repo with at least one commit"; reuse that wording.

## Acceptance

Integration test: `init` in an empty `git init` directory prints the no-commits wording; `init` in a plain directory prints the not-a-repository wording.
