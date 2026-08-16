---
title: built-in guard fires on its own command names inside a quoted argument
status: todo
priority: null
assignee: null
due_date: null
labels:
- guard
- builtin
depends_on: []
created: 2026-08-16T13:57:16Z
updated: 2026-08-16T13:57:16Z
---

A tisket search whose regex named the privileged commands was refused. The pattern anchors on a command position, and a quoted argument to another binary is not one. Match the invoked program the way the mass-stage guard anchors on git, so a mention inside quotes to a different binary passes.
