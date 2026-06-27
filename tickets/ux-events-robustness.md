---
id: ux-events-robustness
title: "events: silent empty on bad filter and in a non-git dir"
status: done
priority: p3
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux]
---
`events --type bogus` / `--ticket ghost` return exit 0 empty (typos silently masked, no validation against known kinds/tickets); and in a non-git dir events silently returns {events:[]} exit 0 while claim/status fail loudly — a tailing consumer never learns git is missing.
Fix: validate --type/--ticket; signal a missing git repo (git:false or an error) instead of empty success.
Found by: scripting agent.
