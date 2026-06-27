---
id: ux-validate-scope-references
title: Undefined/typo'd scopes silently accepted
status: done
priority: p2
dependencies: []
scopes: [core]
paths: []
tags: [ux, bug]
---
A ticket can declare a scope not in ticketsplease.toml; create exit 0, lint ok. It only surfaces later as a baffling guard CONFLICT (declared `my-coer`, affected `my-core`, UNDER-DECLARED) with no hint the declared scope is undefined.
Fix: lint rule for unknown scope references (mirror the dangling-dependency rule); optionally warn at create/set.
Found by: onboarding + editing agents.
