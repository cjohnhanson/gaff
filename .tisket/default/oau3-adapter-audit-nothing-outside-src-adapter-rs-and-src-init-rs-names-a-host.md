---
title: 'adapter audit: nothing outside src/adapter.rs and src/init.rs names a host'
status: todo
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-08-16T22:22:59Z
updated: 2026-08-16T22:22:59Z
---

## Goal

A test that fails when any source file outside `src/adapter.rs` and `src/init.rs` (the host registration layer) contains a host name or a host-specific path or variable: `claude`, `.claude/`, `CLAUDE_CODE`, and the same for every adapter added later. The check is the durable form of the rule "agent-agnostic, with Claude Code as one adapter"; the 2026-08-16 leak (session env, doctor scopes, error text, default host) walked past code review three times.

## Also

`init.rs` exposes `SETTINGS_PATH` and `HOOK_EVENTS` as Claude Code constants for its own tests; fold them into the adapter or mark them test-only.
