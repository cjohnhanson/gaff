# 🪝 gaff

> A gaff is a pole with a sharp hook on the end. It lands what's
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
- **Handlers** — external commands whose output becomes context, on a
  cadence. They live only in the user-scoped config, and a repo must be
  trusted with `gaff trust` before any command runs in it.
- **Git hooks** — gaff writes the scripts in `.git/hooks/`, and they
  call back into gaff. One config declares the agent side and the git
  side. A hook gaff did not write is kept and called first.
- **Profiles** — named overlays that select which entries are active and
  override their cadences. A transition policy states which profiles an
  agent may select for itself. Profiles are advisory: gaff blocks
  nothing.

## Where config lives

`$HOME/.config/gaff/gaff.yml` holds what you want in every repo.
`.gaff/gaff.yml` holds what belongs to one repo, and it wins the names
it shadows. A repo never widens the profiles an agent may grant itself.
Handlers live only in `$HOME/.config/gaff/handlers.yml`.

## What gaff is not

- **Not an agent-hook dispatcher.** The harness's own hook system owns
  the matching, the timeouts, and the parallelism there. gaff registers
  as one handler. gaff does dispatch its own git hooks, because git has
  no dispatcher of its own.
- **Not an enforcement layer.** gaff blocks nothing. It injects context
  only on the events whose output channel is the model's session
  framing. It never decorates a tool result.
- **Not a way to run repo-declared code.** The repo-level config is
  data: sections, text, and cadences. A handler's command can only be
  declared in the user-scoped config. Note the limit of that claim. A
  handler's command still *runs in* the repo's working directory, and
  tools like `git`, `make`, and `just` read executable settings from
  there. Handlers are therefore deny-by-default, and they need
  `gaff trust` per repo.

## Using it

```
gaff init                          # register the hooks in the host's settings file
gaff remind "check CI" --after 10  # one-shot, N tool calls into the future
gaff status --session <id>         # counters, pending entries, one-shots
gaff check                         # validate .gaff/gaff.yml
gaff doctor                        # what is live in this clone
gaff init --git                    # write the git hook scripts
gaff trust                         # allow handlers to run in this repo
gaff check --handlers              # validate ~/.config/gaff/handlers.yml
gaff profile list                  # the declared profiles and who may set them
gaff profile set focus             # switch, and re-prime the sections
gaff log                           # what gaff injected into this session
gaff docs getting-started          # the bundled documentation
```

## Status

These parts work: counters deduped by `tool_use_id`, cadence reminders,
one-shot reminders with a compaction re-arm, prime sections with a
mid-session refresh, profiles with a transition policy, the injection
audit trail (`gaff log`), byte-capped injection with attribution
prefixes, the `init`, `check`, `doctor`, `profile`, and `log` commands,
and the bundled docs.

Claude Code is the only implemented host adapter. The adapter is a
seam, not a hard-coded path. A host declares its payload mapping, its
event names, and its settings path in `src/adapter.rs`. Nothing else in
gaff changes. gaff does not ship a guessed schema for a host
nobody has tested.

A missouri state-graph suite of 15 paths and the cargo unit tests cover
this. The suite's error-surface path checks that every failure exits 1,
never the blocking code 2.
