---
title: "gaff check accepts four runtime-dead configs; README documents unbuilt features"
status: todo
priority: 2
assignee:
labels: [bug, docs]
depends_on: []
created: "2026-08-13T02:06:03Z"
updated: "2026-08-13T02:06:03Z"
---

check passes: unknown keys (no deny_unknown_fields), max_inject_bytes: 0, '/' in an entry name, and a section body over the 4KiB cap (the first thing a real conventions file hits). Separately the README claims Profiles with a transition policy, 'turns' counters, and tool-name filters — none exist in code — and says cargo tests enforce never-exit-2, but main.rs has zero tests and no test spawns the binary. Fix check; cut the false README claims or build the features.
