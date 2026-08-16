---
title: rig-claude-code cannot spawn a hooked Claude Code child
provenance: agent:summary
tags:
- rig-claude-code
- claude-code
- hooks
- gaff
links: []
created: 2026-08-16T14:06:26Z
updated: 2026-08-16T14:06:26Z
---

The crate's invocation always carries `--tools ""` and `--setting-sources ""`, and both are in its owned-flags list, so `with_args` cannot override them (src/request.rs LEAN_FLAGS, src/model.rs OWNED_FLAGS). Live on claude 2.1.233: with those flags the model hallucinated a tool block, no hook fired, and no gaff state dir was created. The same command without them created state, counted a tool call, and injected a SessionStart section.

That is correct for the crate's purpose, a rig completion model, and exactly wrong for a child that must run tools under hooks. Anything that wants a hooked child spawns `claude -p` itself.
