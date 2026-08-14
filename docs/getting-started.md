# Getting started

gaff keeps context alive in a long coding-agent session. Context injected
at session start decays as the conversation grows. It moves into the
low-attention middle of the model's context window. gaff counts what
causes that decay. It counts the tool calls and the prompts. It then
re-injects the important text on a cadence.

## Install the hooks

Run this from the repo root:

    gaff init

The command registers `gaff hook` for five events in
`.claude/settings.local.json`, a local file that git ignores. The events
are SessionStart, UserPromptSubmit, PostToolUse, PostToolUseFailure, and
PostToolBatch. Run `gaff init --uninstall` to remove exactly those
entries.

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

A section is a file under `.gaff/`. gaff injects the whole file at
session start. gaff injects it again when its refresh cadence crosses. A
reminder is one line of text on a cadence. Everything in this file is
data. gaff never runs anything that a repo declares.

## Schedule a one-shot from inside a session

An agent, or you, can reach forward in time:

    gaff remind "check whether the CI run finished" --after 10

Ten counted tool calls later, the reminder appears at the next safe
injection point. It carries the prefix `[gaff:remind]`. It fires once. It
re-arms after a context compaction, because the compaction erases what
the reminder already delivered.

## Inspect

    gaff status --session <id>    # counters, pending entries, one-shots
    gaff check                    # validate .gaff/gaff.yml
    gaff doctor                   # what is live in this clone
