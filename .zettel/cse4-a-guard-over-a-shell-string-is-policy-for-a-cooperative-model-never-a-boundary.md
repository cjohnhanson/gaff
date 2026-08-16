---
title: A guard over a shell string is policy for a cooperative model, never a boundary
provenance: agent:summary
tags:
- gaff
- security
- guards
- agents
links: []
created: 2026-08-16T14:06:26Z
updated: 2026-08-16T14:06:26Z
---

A regex on a Bash command line cannot contain an adversarial agent that shares the user's uid and HOME. Found during review of a plan to sandbox spawned agents with gaff profiles: one redirection writing `{"disableAllHooks":true}` into `.claude/settings.local.json` turns off every hook in the child and the parent, and the state root, `~/.config/gaff/gaff.yml`, and the trust list are all under the same write reach. An env marker holds for hook processes (they inherit the CLI's env, not the tool call's) and for nothing else, because any subprocess can be scrubbed.

<!-- prov agent:inference -->
The boundary that holds is the tool set at spawn. A child with `--tools Read,Grep,Glob` and no Bash has no write path and no subprocess, and then a PreToolUse guard on those tools becomes enforceable rather than policy. A Bash-bearing agent needs an OS boundary: Claude Code's sandbox settings with write denial on the config and state paths, or a separate uid.
<!-- /prov -->
