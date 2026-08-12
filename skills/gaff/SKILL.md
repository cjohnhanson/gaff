---
name: gaff
description: Keep context alive in long coding-agent sessions with gaff — cadence-based reminders, self-scheduled one-shots (gaff remind), prime sections that refresh mid-session, and session counters. Use when you work in a repo with a .gaff/ directory, when you must remember something N tool calls from now, or when you set up context re-injection for a project.
---

# gaff

gaff counts the events of your session and re-injects context on
cadences. Whatever you read at session start has decayed by tool call
200.

Text with the prefix `[gaff:...]` in your context comes from gaff. It is
a section, a reminder, or a one-shot. Treat it as system framing, not as
tool output.

## Schedule a reminder for your future self

This is the most useful command here. Use it when you notice something
that matters later but not now:

    gaff remind "check whether the CI run finished" --after 10

The reminder fires once, about 10 counted tool calls later, at the next
safe injection point. gaff resolves the session from
CLAUDE_CODE_SESSION_ID. Use the command when you start something slow,
such as CI, a build, or a deploy. Use it when you defer a cleanup. Use it
when a task has a step you may forget under context pressure.

A consumed one-shot re-arms after a context compaction. Expect a
delivered reminder to appear once more. This is by design: the compaction
erased what the reminder told you.

## Inspect state

    gaff status --session $CLAUDE_CODE_SESSION_ID

The command prints JSON: `tool_calls`, `prompts`, `pending` (the armed
recurring entries), and `oneshots` (with their `fired` flags).

## Set up a repo

    gaff init          # register the hooks in .claude/settings.local.json
    gaff check         # validate .gaff/gaff.yml
    gaff doctor        # what is live in this clone

The repo config `.gaff/gaff.yml` holds data only. It declares sections,
which are files injected at session start and refreshed on a cadence, and
reminders, which are one-liners on a cadence. Run `gaff docs
configuration` for the full reference.

## Rules

- Never edit a file under the gaff state directory by hand. Use the CLI.
- gaff blocks nothing. If gaff misbehaves, run `gaff doctor` and read the
  stderr warnings. Do not remove the hooks mid-task.
