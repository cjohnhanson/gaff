<!-- metadata
title: "Getting started with gaff"
description: "Install gaff, wire the hooks, and watch a section fire"
type: tutorial
-->

# Getting started

gaff keeps context alive in a long coding-agent session. Context injected
at session start decays as the conversation grows, and moves into the
low-attention middle of the model's context window. gaff counts what
causes that decay, the tool calls and the prompts, and re-injects the
text you named on a cadence.

## Install the hooks

Run this from the repo root:

    gaff init

The command registers `gaff hook` for seven events in
`.claude/settings.local.json`, a local file that git ignores. The events
are SessionStart, UserPromptSubmit, PreToolUse, PostToolUse,
PostToolUseFailure, PostToolBatch, and Stop. Run `gaff init --uninstall`
to remove exactly those entries.

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
session start, and again when its refresh cadence crosses. A reminder is
one line of text on a cadence. Everything in this file is data, and gaff
never runs anything a repo declares.

## Schedule a one-shot from inside a session

An agent, or you, can schedule a reminder for later in the session:

    gaff remind "check whether the CI run finished" --after 10

Ten counted tool calls later, the reminder appears at the next safe
injection point, under the prefix `[gaff:remind]`. It fires once, and it
re-arms after a context compaction, because the compaction erases what
the reminder already delivered.

## Inspect

    gaff status --session <id>    # counters, pending entries, one-shots
    gaff check                    # validate .gaff/gaff.yml
    gaff doctor                   # what is live in this clone
    gaff prime                    # what gaff is, for an agent's context
