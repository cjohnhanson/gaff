# 🪝 gaff

> A gaff is a pole with a sharp hook on the end — used to land what's
> drifting past.

gaff is a context-lifecycle handler for coding agents. It counts the hook
events of a session. It re-injects context on a cadence. It delivers
prime sections and advisory profiles.

## The problem

Context injected at session start decays as the conversation grows. It
moves into the low-attention middle of the context window. The decay
follows the number of messages and tool calls, not the wall clock.

The agent harness re-delivers no context on a cadence. Rules and skills
load on conditions. Reminders do not exist. A session of 300 tool calls
ends with its opening instructions effectively invisible.

## What gaff does

- **Counters** — per-session tallies over the hook events: prompts and
  tool calls. gaff keeps them in an append-only ledger, and a tool call
  counts once across its Pre, Post, and failure events.
- **Cadences** — re-inject a text every N tool calls or prompts. An
  agent can also schedule a one-shot reminder N tool calls into its own
  future. gaff re-arms a one-shot after a context compaction.
- **Prime sections** — the session-start context, split into sections.
  Each section refreshes on its own cadence.

## What gaff is not

- **Not a hook dispatcher.** The harness's own hook system owns the
  matching, the timeouts, and the parallelism. gaff registers as one
  handler.
- **Not a git-hook manager.** lefthook and pre-commit already do that
  work well.
- **Not an enforcement layer.** gaff blocks nothing. It injects context
  only on the events whose output channel is the model's session
  framing. It never decorates a tool result.
- **Not a way to run repo code.** The repo-level config is data:
  sections, text, and cadences. Anything executable lives in the
  user-scoped config.

## Using it

```
gaff init                          # register the hooks (.claude/settings.local.json)
gaff remind "check CI" --after 10  # one-shot, N tool calls into the future
gaff status --session <id>         # counters, pending entries, one-shots
gaff check                         # validate .gaff/gaff.yml
gaff doctor                        # what is live in this clone
gaff docs getting-started          # the bundled documentation
```

## Status

These parts work: counters deduped by `tool_use_id`, cadence reminders,
one-shot reminders with a compaction re-arm, prime sections with a
mid-session refresh, byte-capped injection with attribution prefixes,
the `init`, `check`, and `doctor` commands, and the bundled docs.

A missouri state-graph suite of 15 paths and the cargo unit tests cover
this. The suite's error-surface path checks that every failure exits 1,
never the blocking code 2. Profiles are not built yet.
