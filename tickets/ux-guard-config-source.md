---
id: ux-guard-config-source
title: guard reads config + open-ticket set from working tree, not --base
status: done
priority: p1
dependencies: []
scopes: [core, cli]
paths: []
tags: [ux, bug, scripting]
---
guard takes the file DIFF from git refs correctly but reads [scopes] mapping and the other open tickets from whatever is CHECKED OUT. Running guard on the feature branch (or in CI after `checkout pr-branch`) uses the branch's (possibly stale/empty) config and can give a FALSE all-clear.
Repro: scope map committed on main, empty on the branch -> `guard tkt/x --base main` is conflict:true exit 6 from main's worktree, but conflict:false exit 0 from the branch's worktree.
Also: with the default empty [scopes], guard is always a no-op (affected_scopes:[], exit 0) with NO warning that scope mapping is unconfigured.
Fix: read [scopes] + open-ticket set from --base (or an explicit --config-ref); warn/fail loudly when [scopes] is empty.
Found by: scripting agent. (Related to the deferred actual-vs-actual collision work.)
