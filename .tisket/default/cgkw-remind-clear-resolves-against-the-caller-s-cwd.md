---
title: remind --clear resolves against the caller's cwd
status: todo
priority: null
assignee: null
due_date: null
labels:
- remind
- state
depends_on: []
created: 2026-08-16T13:57:16Z
updated: 2026-08-16T13:57:16Z
---

State is keyed by cwd hash. Run from a directory other than the one the session's hooks run in, --clear finds no hold under that root, removes nothing, and prints released. It should resolve the session's real root (the session id is enough to find it) or refuse when no hold exists.
