---
title: 'doctor: report git-hook install state per declared entry'
status: todo
priority: null
assignee: null
due_date: null
labels:
- doctor
- git
depends_on: []
created: 2026-08-16T13:57:16Z
updated: 2026-08-16T13:57:16Z
---

gaff doctor lists config, state, agent hooks, guards, handlers, and nothing for git: entries. The user config declared a gitleaks pre-commit entry for a day while no repo had it installed, and nothing said so. Add a git: line: each declared entry, and for each hook it names, whether .git/hooks/<hook> is gaff's script, a foreign hook, or absent.
