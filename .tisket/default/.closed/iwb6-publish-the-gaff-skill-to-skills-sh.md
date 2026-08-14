---
title: publish the gaff skill to skills.sh
status: done
priority: '4'
assignee: null
due_date: null
labels:
- skills
depends_on: []
created: '2026-08-12T17:01:38Z'
updated: '2026-08-14T21:57:07Z'
---

skills/gaff/SKILL.md ships in-repo. Publish to the skills.sh ecosystem per the codelikecody convention for consumer-facing skills.

## Scratch Notes

Research (zettel ozvq in codelikecody): skills.sh has no submission flow — public git repos with skills/ dirs are discovered via install telemetry. This repo already qualifies. Remaining action is verification only: npx skills add cjohnhanson/gaff and confirm the skill installs.

CLOSED as invalid 2026-08-14. Cody asked who requested this; nobody did. It was self-seeded in commit 849b76e ('seed follow-up issues'), not requested.

The issue also refutes itself: its own research notes record that skills.sh has no submission flow, so there is no publish action. The only step left was 'npx skills add' as a verification, and that npx skills reference is the same thing Cody rejected as slop in the almanac README.
