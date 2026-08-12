# Configuration reference

The repo config lives at `.gaff/gaff.yml`. It holds data only: text, file
paths, and cadences. gaff executes nothing in it.

## Top-level fields

| Field | Default | Description |
|-------|---------|-------------|
| `sections` | `[]` | The prime sections (see below) |
| `reminders` | `[]` | The recurring reminders (see below) |
| `max_inject_bytes` | `4096` | The hard cap per flush, truncation marker included |

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

## Exit codes

Every gaff invocation exits 0 for success or 1 for an internal error. It
never exits 2, the code that blocks an agent session. A broken config
prints a warning on stderr, writes a `degraded` marker in the state
directory, and gaff continues without reminders. Run `gaff doctor` to see
the degradation.
