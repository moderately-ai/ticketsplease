---
id: ux-no-config-error
title: Pre-init commands leak a raw OS error
status: done
priority: p2
dependencies: []
scopes: [cli, core]
paths: []
tags: [ux, onboarding]
---
Running any command before init: `error: invalid input: cannot read .../ticketsplease.toml: No such file or directory (os error 2)` exit 3.
Fix: detect the missing config and emit 'not initialized — run `tkt init`' (reconsider exit code).
Found by: onboarding agent.
