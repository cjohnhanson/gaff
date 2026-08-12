# 🪝 gaff

> A gaff is a pole with a sharp hook on the end — used to land what's
> drifting past.

Context-lifecycle handler for coding agents. Counters over the hook event
stream, cadence-driven context re-injection, prime sections, and advisory
profiles.

## The problem

Context injected at session start decays as the conversation grows — it
slides into the low-attention middle of the context window. Decay is a
function of messages and tool calls, not wall clock time. Nothing in the
agent harness re-delivers context on a *cadence*: rules and skills load on
conditions, reminders don't exist, and a 300-tool-call session ends with
its opening instructions effectively invisible.

## What gaff does

- **Counters** — per-session tallies over hook events (prompts, turns,
  tool calls filtered by name), kept as an append-only ledger.
- **Cadences** — "every 20 tool calls, re-inject X"; one-shot "after N"
  reminders an agent can schedule into its own future; re-armed after
  context compaction.
- **Prime sections** — session-start context decomposed into sections,
  each individually refreshable on its own cadence.
- **Profiles** — named overlays (which sections, which cadences) with a
  transition policy for which switches an agent may perform on itself.
  Advisory by design; real enforcement belongs to managed settings.

## What gaff deliberately is not

- **Not a hook dispatcher.** The harness's native hook system owns
  matching, timeouts, and parallelism; gaff registers as one handler.
- **Not a git-hook manager.** lefthook and pre-commit exist and are good.
- **Not an enforcement layer.** gaff blocks nothing. It injects context
  on exactly the events whose output channel is the model's session
  framing, and never decorates tool results.
- **Not configured to execute repo code.** Repo-level config is data
  (sections, text, cadences). Anything executable lives in user-scoped
  config.

## Using it

```
gaff init                          # register hooks (.claude/settings.local.json)
gaff remind "check CI" --after 10  # one-shot, N tool calls into the future
gaff status --session <id>         # counters, pending, one-shots
gaff check                         # validate .gaff/gaff.yml
gaff doctor                        # what's live in this clone
gaff docs getting-started          # bundled documentation
```

## Status

Working: counters (deduped by tool_use_id), cadence reminders, one-shot
reminders with compaction re-arm, prime sections with mid-session
refresh, byte-capped injection with attribution prefixes, init/check/
doctor, bundled docs. Tested by a missouri state-graph suite (15 paths)
plus cargo unit tests; the never-exit-2 invariant is enforced by both.
