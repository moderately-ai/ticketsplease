---
id: ux-reclaim-event-noise
title: Idempotent re-claim still logs a duplicate claim event
status: done
priority: p3
dependencies: []
scopes: [core]
paths: []
tags: [ux]
---
Re-claiming as the current holder is a no-op (stolen:false) but still appends a claim event, so the append-only log accrues reclaim noise.
Fix: don't emit a claim event when the claim is an unchanged renewal (or mark it kind=renew).
Found by: orchestration agent.
