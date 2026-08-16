---
title: 'check: report the user layer''s SessionStart byte total against max_inject_bytes'
status: todo
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-08-16T19:33:18Z
updated: 2026-08-16T19:33:18Z
---

## Goal

`gaff check` (or `gaff doctor`) states the byte total of everything the user layer injects at SessionStart — every section body plus entry headers — next to `max_inject_bytes`, and flags it when the total is over the cap.

## Why

The prime contract's assembly injects six user sections at SessionStart (voice, four captured primes, ecosystem). Today the only way to know the flush fits under the cap is to add the file sizes by hand. A section that grows past the budget is silently held back or tail-cut, and the symptom is a section that stops arriving.

## Scope

- Sum the SessionStart flush for the user layer: section bodies + `[gaff:<name>]` headers + separators, the same arithmetic the engine uses.
- Print it in `doctor` beside the cap. `check --handlers` (the user-config check) exits 1 when it exceeds the cap.
- Repo layer: report the same for the combined flush when run inside a repo.

## Not in scope

Changing the tail-cut order or the cap default.
