---
title: Inline --settings registers Claude Code hooks with every file source dropped
provenance: agent:summary
tags:
- claude-code
- hooks
- settings
- gaff
links: []
created: 2026-08-16T14:06:26Z
updated: 2026-08-16T14:06:26Z
---

`claude -p --setting-sources "" --settings '<json>'` where the JSON carries only a hook registration: the child's tool call reached the hook and wrote state. The same run without `--settings` wrote nothing. Verified live, claude 2.1.233, 2026-08-15.

So a spawned child's hook registration can come from an argument the spawner controls, with no file the child can see deciding whether hooks run. The child then inherits no user hooks, MCP servers, plugins, CLAUDE.md, or repo `.claude` settings. Three related checks also held: an outside-cwd Read in `-p` under default permission mode is denied (a prompt is a denial); a Grep pattern beginning with `--` stays a pattern; a repo `.claude/settings.json` `env` block does not reach the child's hook processes.
