---
title: "gaff log: per-session injection audit trail"
status: todo
priority: 3
assignee:
labels: [observability]
depends_on: []
created: "2026-08-12T17:01:38Z"
updated: "2026-08-12T17:01:38Z"
---

Design-review commitment: a structured record per handler/flush (session, event, bytes, what was injected) with size-bounded rotation, and a gaff log --session <id> view reconstructing what was injected where. The attribution prefixes make transcripts readable; this makes them queryable.
