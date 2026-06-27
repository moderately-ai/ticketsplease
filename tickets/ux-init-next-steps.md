---
id: ux-init-next-steps
title: init gives no next-steps and no non-git warning
status: done
priority: p2
dependencies: []
scopes: [cli]
paths: []
tags: [ux, onboarding]
---
init prints only what it created — no guidance to define [scopes], create a first ticket, or read the bundled skill; and it succeeds in a non-git dir with no warning even though claim/guard/status then fail on missing git.
Fix: print a short next-steps block; detect a non-git dir and warn that claim/guard/status need git init + a commit.
Found by: onboarding agent.
