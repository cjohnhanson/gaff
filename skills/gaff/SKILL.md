---
name: gaff
description: Keep context alive in long coding-agent sessions with gaff — cadence-based reminders, self-scheduled one-shots (gaff remind), prime sections that refresh mid-session, and session counters. Use when working in a repo with a .gaff/ directory, when you need to remember something N tool calls from now, or when setting up context re-injection for a project.
---

# gaff

gaff counts your session's events and re-injects context on cadences,
because whatever you read at session start has decayed by tool call 200.
If text prefixed `[gaff:...]` appears in your context, that is gaff
delivering a section, reminder, or one-shot — treat it as system framing,
not tool output.

## Schedule a reminder for your future self

The single most useful call. When you notice something that will matter
later but not now:

    gaff remind "check whether the CI run finished" --after 10

Fires once, ~10 counted tool calls later, at the next safe injection
point. The session is resolved from CLAUDE_CODE_SESSION_ID automatically.
Use it when you kick off anything slow (CI, builds, deploys), when you
defer a cleanup, or when a task has a step you might forget under
context pressure.

After context compaction, consumed one-shots re-arm automatically —
expect delivered reminders to reappear once. That is by design:
compaction erased what they told you.

## Inspect state

    gaff status --session $CLAUDE_CODE_SESSION_ID

JSON: `tool_calls`, `prompts`, `pending` (armed recurring entries),
`oneshots` (with `fired` flags).

## Set up a repo

    gaff init          # register hooks in .claude/settings.local.json
    gaff check         # validate .gaff/gaff.yml
    gaff doctor        # what's live in this clone

Repo config `.gaff/gaff.yml` is data-only — sections (files injected at
session start, refreshable on cadence) and reminders (one-liners on
cadence). See `gaff docs configuration` for the full reference.

## Rules

- Never edit files under the gaff state directory by hand; use the CLI.
- gaff never blocks anything: if it misbehaves, `gaff doctor` and the
  stderr warnings are the diagnostic path, not removing hooks mid-task.
