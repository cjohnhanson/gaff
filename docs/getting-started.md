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

```console
$ gaff init
gaff hooks registered in .claude/settings.local.json

```

The command registers `gaff hook` for seven events in
`.claude/settings.local.json`, a local file that git ignores. The events
are SessionStart, UserPromptSubmit, PreToolUse, PostToolUse,
PostToolUseFailure, PostToolBatch, and Stop. Run `gaff init --uninstall`
to remove exactly those entries.

## Declare what to keep alive

Create `.gaff/gaff.yml`:

```yaml
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
```

A section is a file under `.gaff/`. gaff injects the whole file at
session start, and again when its refresh cadence crosses. A reminder is
one line of text on a cadence. Everything a hook reads from this file is
data; a `git:` or `github:` entry names a command, and that runs only
after you run `gaff init --git` or `gaff init --github` yourself.

## Schedule a one-shot from inside a session

An agent, or you, can schedule a reminder for later in the session.
Inside a session, the host sets the session id. From a terminal, pass
it with `--session`:

```console
$ gaff remind "check whether the CI run finished" --after 10 --session demo

```

Ten counted tool calls later, the reminder appears at the next safe
injection point, under the prefix `[gaff:remind]`. It fires once, and it
re-arms after a context compaction, because the compaction erases what
the reminder already delivered.

## Inspect

`gaff status` prints the counters, the pending entries, and the
one-shots. `gaff check` validates the config, `gaff doctor` shows what
is live in this clone, and `gaff prime` prints what gaff is, for an
agent's context:

```console
$ gaff status --session demo
{"oneshots":[{"at":10,"fired":false,"id":"r1"}],"pending":[],"prompts":0,"tool_calls":0}

$ gaff check
config ok: 1 reminder(s), 1 section(s), 0 guard(s), cap 4096 bytes

$ gaff doctor
config:  ok (1 reminder(s), 1 section(s))
state:   [..]/.local/state/gaff/[..]
hooks:   registered (local (7)), all 7 events
guards:  none active
         (a broken or unreadable user config disarms every guard)
handlers: none declared

$ gaff prime
# gaff
a context-lifecycle handler for coding agents
From the host's hooks, gaff runs guards on tool calls, injects sections and reminders at session start and on a cadence, and can hold the stop until a reminder clears. Each injected entry opens with a tag, gaff:<name> in square brackets. A refused tool call names its guard. Repo config is .gaff/gaff.yml; user config is $HOME/.config/gaff/gaff.yml.
Commands:
  gaff doctor
  gaff status
  gaff remind <text> (--after <n> | --at stop) [--id <id>]
  gaff remind --clear --id <id>
  gaff check
More: gaff --help; gaff docs

```
