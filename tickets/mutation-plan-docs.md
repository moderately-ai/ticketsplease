---
id: mutation-plan-docs
title: "skill/docs: transactional batch + explicit-id graph rule"
status: done
priority: p2
dependencies: [mutation-create-from-wire]
related: []
scopes: [skill, docs]
shared_scopes: []
paths: []
tags: [mutation-plan, docs]
---
## Initiative

Tag: `mutation-plan`. Invariants: see `mutation-plan-types`.

## Goal

Make agent- and human-facing docs match true transactional create and the locked authoring policy — no more validate-only “all-or-nothing” lies.

## Gap

SKILL.md / parallel-workflow.md / commands.md claim batch create is all-or-nothing while write was sequential. Auto-id + intra-batch deps policy is undocumented. README barely mentions `--from`. `--no-validate` vs cycles not aligned with set/link in docs.

## Work

- `crates/ticketsplease-cli/skill/SKILL.md` — transactional language: on failure no partial creates; dry-run shows final ids.
- `references/commands.md` create section — plan/final ids; cycle always checked; `--no-validate` scope; exit codes.
- `references/parallel-workflow.md` — **explicit `id` on every graph member** in a batch; re-run safe after failed batch.
- `README.md` — one-liner `tkt create --from …`.
- `cli.rs` CreateArgs help if field list is incomplete (`shared_scopes` / `template`).
- Grep skill for remaining false multi-file “atomic” claims; fix only create/set/rename durability claims that this initiative has shipped (at least create after this ticket's dep).

## Acceptance

- Docs checklist: no unqualified “all-or-nothing” for validate-only semantics; explicit-id graph rule present; dry-run described as plan preview; recipe multi-step **not** claimed transactional.
- Agents reading skill get the same contract as `mutation-create-from-wire` behavior.

## Non-goals

- Implementing remaining multi-write surfaces.
- schema_version bump docs for create JSON.

## Refs

Depends on create wire so docs describe shipped behavior.
