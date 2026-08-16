---
title: A tool's prime describes the tool; policy about using it belongs to the assembler
provenance: agent:summary
tags:
- prime
- agent-context
- design
links: []
created: 2026-08-16T19:33:51Z
updated: 2026-08-16T19:33:51Z
---

A prime is a tool's self-description for an agent's context: what it is, its model, the facts the binary enforces, and the commands an agent reaches for. It is a pure function of the binary, under 700 bytes, and it directs nothing: no always, no never, no workflow, no session-start ritual, no sibling tool, no location claim. Those are policy, and policy belongs to whoever assembles the context (a gaff section, a reminder, a CLAUDE.md), because the same prime bytes reach every host and user. tisket's earlier prime broke this in three ways: it said 'this repository uses tisket' (false for a user-level tracker), printed a workflow (policy), and appended additional_instructions from config (a policy slot in the binary). The contract lives in mdstore's tracker, issue dxyx; built 2026-08-16 in tisket, zettel, gaff, almanac.
