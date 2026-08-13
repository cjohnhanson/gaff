---
title: "concurrent hooks corrupt the ledger and lose cadence crossings"
status: done
priority: 1
assignee:
labels: [bug, concurrency]
depends_on: []
created: 2026-08-13T02:06:03Z
updated: "2026-08-13T17:34:15Z"
---

src/state.rs:108 writeln! on a bare file is not atomic per line; counting reads-then-appends with no lock. Claude Code fires tool hooks in parallel. 20 concurrent gaff hook calls against cadence 20 failed to arm in 5 of 10 rounds and left interleaved garbage lines. The core function fails ~half the time under the normal case. Fix: O_APPEND writes under PIPE_BUF, or flock; the ops design already specified this and the impl dropped it.
