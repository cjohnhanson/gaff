# Configuration reference

The repo config lives at `.gaff/gaff.yml`. It holds data only: text, file
paths, and cadences. gaff executes nothing in it.

## Top-level fields

| Field | Default | Description |
|-------|---------|-------------|
| `sections` | `[]` | The prime sections (see below) |
| `reminders` | `[]` | The recurring reminders (see below) |
| `max_inject_bytes` | `4096` | The hard cap per flush, truncation marker included |
| `profiles` | `{}` | The named overlays (see below) |
| `default_profile` | none | The profile that applies when nothing else selects one |
| `transitions` | `{}` | Which profiles an agent may select for itself |

## Sections

    sections:
      - name: conventions          # unique across sections AND reminders
        file: sections/conv.md     # relative to .gaff/
        refresh:                   # optional; omit for session start only
          tool_calls: 25

gaff injects every section in full at SessionStart, in config order, with
the prefix `[gaff:<name>]`. A `refresh` cadence injects the section again
mid-session, when the counter crosses.

## Reminders

    reminders:
      - name: scratch
        every:
          tool_calls: 20           # or prompts: N
        text: "One line of text."

gaff emits the reminder as `[gaff:<name>] <text>` when the cadence
crosses, at the next safe injection point.

## Profiles

A profile is a named overlay on the entries the config already
declares. A profile never adds an entry. It selects from the declared
entries and may override their cadences, so one file still states
everything a repo can inject.

```yaml
profiles:
  focus:
    only: [build-clean]        # keep only these entries
    cadence:
      build-clean: {tool_calls: 2}   # override the cadence
  quiet:
    disable: [chatty]          # drop these entries
    max_inject_bytes: 512      # a tighter cap under this profile
default_profile: focus
transitions:
  agent_may_set: [focus]       # every other profile is human-only
```

| Field | Default | Description |
|-------|---------|-------------|
| `only` | every entry | Keep only the named entries |
| `disable` | `[]` | Drop the named entries; applied after `only` |
| `cadence` | `{}` | Cadence overrides, keyed by entry name |
| `max_inject_bytes` | the base cap | The cap under this profile |

The resolution path is `GAFF_PROFILE`, then the session state, then a
`.gaff/profile` file, then `default_profile`. The first hit wins. An
unknown name applies nothing and warns, because a typo must never
silently empty the config.

A switch re-primes: the next flush delivers every section rather than
wait for each refresh cadence.

### The transition policy

Profiles are advisory. gaff blocks nothing, and an agent that can write
files can edit this config regardless. The policy states intent and
refuses the agent-facing path, so a switch an operator did not sanction
is at least not a supported one. gaff decides who is asking by
structural identity: a terminal on stdin is a human, anything else is an
agent. A profile absent from `transitions.agent_may_set` is human-only.

## Cadence units

- `tool_calls` — counted on PostToolUse and PostToolUseFailure, and
  deduplicated by `tool_use_id`. A call and its failure count once.
- `prompts` — counted on UserPromptSubmit.

## Injection points and the byte cap

gaff injects only on SessionStart, UserPromptSubmit, and PostToolBatch.
The harness delivers the output of those events as session framing. gaff
never decorates PostToolUse. That context attaches to the tool result,
and the model reads it as tool output.

A threshold that crosses on an unsafe event arms the entry. The delivery
waits for the next safe event. Per flush, gaff merges the sections first,
then the reminders in config order, then the one-shots by id. A blank
line separates each entry, and the total stays under `max_inject_bytes`.
An entry that does not fit stays armed for the next flush, and gaff
appends `[gaff:truncated]`.

## State

The session state holds the counters, the armed markers, and the
one-shots. It lives outside the repo, under `$XDG_STATE_HOME/gaff/` or
`~/.local/state/gaff/`, keyed by the working directory. It is never
inside the repo, where `git clean -xdf` would erase it mid-session. Set
`GAFF_STATE_DIR` to override the location.

## The injection audit trail

gaff appends one line to `injections.jsonl` in the session state for
every flush it delivers. `gaff log` prints it: the event, the byte
count, and the entry names. The log records what gaff put into a
session, which is the question a reader actually has when a reminder
seems to have fired at the wrong time.

## Host adapters

Claude Code is the only implemented adapter. An adapter owns three
host-specific facts: the payload mapping, the event names, and the
settings path that `gaff init` writes. `gaff hook` selects the adapter
from `GAFF_HOST`, or from the payload shape when that variable is
absent. `gaff init --host <name>` targets a named host.

gaff ships no guessed schema for an untested host. Adding one means
adding an `Adapter` constant with that host's real field names, taken
from its documentation.

## Exit codes

Every gaff invocation exits 0 for success or 1 for an internal error. It
never exits 2, the code that blocks an agent session. A broken config
prints a warning on stderr, writes a `degraded` marker in the state
directory, and gaff continues without reminders. Run `gaff doctor` to see
the degradation.
