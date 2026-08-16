---
title: claude -p is plan-funded; the help centre names it verbatim
provenance: agent:summary
tags:
- claude-code
- billing
- subscription
links: []
created: 2026-08-16T14:06:58Z
updated: 2026-08-16T14:06:58Z
---

Anthropic's Agent SDK plan page lists what a Claude subscription funds, and `claude -p` is on the list by name.

<!-- prov citation src=https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan -->
> Claude Agent SDK usage in your own projects (Python or TypeScript)
> The `claude -p` command in Claude Code (non-interactive mode)
> The Claude Code GitHub Actions integration
> Third-party apps that authenticate with your Claude subscription through the Agent SDK
<!-- /prov -->

The other help-centre page often cited for this, 11145838 ("Use Claude Code with your Pro or Max plan"), says nothing about `-p`; its one condition is that a set `ANTHROPIC_API_KEY` bills the API instead. rig-claude-code's README cites 11145838; the citation should move to 15036540. Fetched 2026-08-15.
