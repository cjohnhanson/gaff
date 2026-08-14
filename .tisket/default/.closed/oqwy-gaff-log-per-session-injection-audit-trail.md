---
title: 'gaff log: per-session injection audit trail'
status: done
priority: '3'
assignee: null
due_date: null
labels:
- observability
depends_on: []
created: '2026-08-12T17:01:38Z'
updated: '2026-08-14T20:32:05Z'
---

Design-review commitment: a structured record per handler/flush (session, event, bytes, what was injected) with size-bounded rotation, and a gaff log --session <id> view reconstructing what was injected where. The attribution prefixes make transcripts readable; this makes them queryable.

## Scratch Notes

DONE: gaff log prints the per-session injection audit trail. Every delivered flush appends one line to injections.jsonl with the event, the byte count, and the entry names. Covered by unit tests and the missouri profiles path.
