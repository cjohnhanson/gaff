---
title: 'guards, stop hooks, the release valve, and gaff.d: the 2026-08-14/15 run'
status: done
priority: null
assignee: null
due_date: null
labels:
- guards
- stop-hook
- retro
depends_on: []
created: 2026-08-16T13:57:16Z
updated: 2026-08-16T13:57:16Z
---

Forty commits over 2026-08-14 and 2026-08-15, filed after the fact. The Working Notes rule says non-trivial work has an issue; this run did not, and an audit found the gap.

## What shipped

- Guards: regex-based tool-call refusal, user-level only; a repo cannot declare or disarm one. Six review rounds, twelve blockers found and closed, among them a repo supplying a user section's body, a pre-push gate failing open behind a kept hook, and `gaff init` destroying a settings file it could not read.
- Stop hooks: Stop is a flush point; `remind --at stop [--times N]` holds a stop; a handler with `blocks: true` gates it on a command's exit.
- The human's one-shot release valve for a guard, refused from any agent Bash call by a built-in guard.
- Heredoc-aware guards: a mention inside a data heredoc is not a call; a heredoc feeding a shell keeps its body.
- co.d/gaff.d: the user config moved to co.d, linked out of store: 8 guards, 2 reminders, the voice section.

## Edges found and not fixed

- `gaff remind --clear` resolves against the caller's cwd, so from the wrong directory it clears nothing and prints "released".
- The built-in guard fires on its own command names inside a quoted argument to another binary.
- `gaff doctor` has no git-hook line, so an uninstalled declared hook is invisible.
