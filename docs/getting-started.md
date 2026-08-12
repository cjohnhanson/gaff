# Getting started

gaff keeps context alive in long coding-agent sessions. Context injected
at session start decays as the conversation grows — it slides into the
low-attention middle of the model's context window. gaff counts what
actually causes that decay (tool calls, prompts) and re-injects what
matters on a cadence.

## Install the hooks

From your repo root:

    gaff init

This registers `gaff hook` for five events in `.claude/settings.local.json`
(local, gitignored): SessionStart, UserPromptSubmit, PostToolUse,
PostToolUseFailure, and PostToolBatch. Run `gaff init --uninstall` to
remove exactly those entries.

## Declare what to keep alive

Create `.gaff/gaff.yml`:

    sections:
      - name: conventions
        file: sections/conventions.md
        refresh:
          tool_calls: 25

    reminders:
      - name: scratch
        every:
          tool_calls: 20
        text: "Update your working notes before they go stale."

Sections are files under `.gaff/` injected in full at session start and
re-injected when their refresh cadence crosses. Reminders are one-liners
on a cadence. Everything in this file is data — gaff never executes
anything a repo declares.

## Schedule a one-shot from inside a session

An agent (or you) can reach forward in time:

    gaff remind "check whether the CI run finished" --after 10

Ten counted tool calls later, at the next safe injection point, the
reminder appears prefixed `[gaff:remind]`. It fires exactly once — and
re-arms after a context compaction, because compaction erases whatever
the reminder already delivered.

## Inspect

    gaff status --session <id>    # counters, pending, one-shots
    gaff check                    # validate .gaff/gaff.yml
    gaff doctor                   # what's live in this clone
