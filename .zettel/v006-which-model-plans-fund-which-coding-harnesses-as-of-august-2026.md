---
title: Which model plans fund which coding harnesses, as of August 2026
provenance: agent:summary
tags:
- billing
- subscription
- harness
- providers
links: []
created: 2026-08-16T14:06:26Z
updated: 2026-08-16T14:06:26Z
---

Two providers gate the subscription behind one first-party harness; two sell subscription-priced quota behind a key any harness can use.

- Anthropic: a Pro/Max plan funds Claude Code, the Agent SDK, `claude -p`, and third-party agents built on the SDK. April 4 cut third-party tools off; mid-May reinstated them; a June 15 credit split was cancelled and usage "keeps drawing from the plan as before". No raw API key against the plan.
- OpenAI: a ChatGPT plan funds Codex only (sign in with ChatGPT). An API key exists but bills a separate Platform account; Plus and Pro include no API credits.
- Zhipu (GLM) and Moonshot (Kimi): a coding plan is quota on a 5-hour and weekly cadence, reachable through a key with an Anthropic-compatible endpoint. Third-party clients are the intended use.

<!-- prov agent:inference -->
So "cross-agent" tooling is operational against Zhipu and Moonshot today, and against Anthropic only through the SDK's auth flow or `claude -p`, never a bare key.
<!-- /prov -->

Sources checked 2026-08-15: OpenAI help centre 11369540 and 9039756; VentureBeat on the May reinstatement; support.claude.com 15036540; vendor plan pages for GLM and Kimi.
