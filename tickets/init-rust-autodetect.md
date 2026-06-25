---
id: init-rust-autodetect
title: Auto-detect Rust workspace in init and seed scopes
status: done
priority: p1
dependencies: []
scopes: [cli]
paths: []
tags: []
---
On init, detect a root Cargo.toml, set language.backend=rust, and pre-populate [scopes]/[scope_crates] from cargo metadata workspace members.
