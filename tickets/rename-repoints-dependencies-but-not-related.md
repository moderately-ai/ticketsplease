---
id: rename-repoints-dependencies-but-not-related
title: rename repoints dependencies but not related, so it creates the dangling link lint then reports
status: done
priority: p2
dependencies: []
related: []
scopes: [cli]
shared_scopes: []
paths: []
tags: [bug, links]
---
`rename` repoints `dependencies` and silently leaves `related` pointing at the old id. Since `lint` flags a dangling `related` as `missing-related`, the tool manufactures a lint violation and then reports it — and `rename`'s own success output ("repointed dependents: …") reads as if every reference was handled.

## Repro

```
$ tkt create --id alpha --title alpha
$ tkt create --id beta --title beta --related alpha --depends-on alpha
$ grep -E '^(dependencies|related):' tickets/beta.md
dependencies: [alpha]
related: [alpha]

$ tkt rename alpha alpha-renamed
Renamed `alpha` -> `alpha-renamed`
  repointed dependents: beta

$ grep -E '^(dependencies|related):' tickets/beta.md
dependencies: [alpha-renamed]
related: [alpha]                 # <-- dangling

$ tkt lint
beta.md (beta) [missing-related]: related to missing ticket `alpha`
error: invalid input: 1 problem(s) found
```

## Cause

`commands.rs::rename` inspects only the dependency edge:

```rust
// Repoint every ticket that depended on the old id.
for mut t in store.load_all()? {
    if t.id == args.new { continue; }
    if t.dependencies.iter().any(|d| d == &args.old) {
        t.remove_dependency(&args.old)?;
        t.add_dependency(&args.new)?;
        store.save(&t)?;
        repointed.push(t.id.clone());
    }
}
```

`Ticket::add_related` / `Ticket::remove_related` already exist in core (`ticket.rs:388,397`), so the fix is symmetric with the dependency arm and needs no new core surface.

## Why it matters beyond the lint noise

This bit us for real in QuiltDB. A rename's own commit message recorded "no dependents, safe rename" — true for dependencies, and the author had no signal that a `related` edge existed. The dangling link sat undetected until an unrelated `tkt lint` run weeks later. `related` is documented as a queryable, graphable cross-reference (`--where related:x`, `graph`'s `related_edges`), so a silently-broken one degrades those surfaces too, not just lint.

## Notes for the fix

- Decide whether `repointed` in the JSON/human output should distinguish the two edge kinds, or just report the union. The current key is `"repointed": [ids]`; a ticket repointed only on its `related` edge is arguably still "repointed". Union is probably right, but it is a schema-visible choice.
- A ticket could hold both edges to the same target (the repro does). It must appear once, not twice.
- `delete` may deserve the same audit: it warns nothing about inbound `related` edges either, though there the dangling link is arguably intended (the target really is gone) and `lint` correctly reports it. Worth a glance, not necessarily a change.

## Done when

`rename` repoints `related` as well as `dependencies`; `lint` is clean immediately after a rename that had both edge kinds pointing at the old id; an integration test covers it alongside the existing `rename_moves_file_and_repoints_dependents`.
