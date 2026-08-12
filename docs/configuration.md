# Configuration reference

Repo config lives at `.gaff/gaff.yml`. It is data only: text, file
paths, cadences. Nothing in it is executed.

## Top-level fields

| Field | Default | Description |
|-------|---------|-------------|
| `sections` | `[]` | Prime sections (see below) |
| `reminders` | `[]` | Recurring reminders (see below) |
| `max_inject_bytes` | `4096` | Hard cap per flush, truncation marker included |

## Sections

    sections:
      - name: conventions          # unique across sections AND reminders
        file: sections/conv.md     # relative to .gaff/
        refresh:                   # optional; omit for session-start only
          tool_calls: 25

Injected in full at SessionStart (all sections, config order), prefixed
`[gaff:<name>]`. A `refresh` cadence re-injects the section mid-session
when the counter crosses.

## Reminders

    reminders:
      - name: scratch
        every:
          tool_calls: 20           # or prompts: N
        text: "One line of text."

Emitted as `[gaff:<name>] <text>` when the cadence crosses, at the next
safe injection point.

## Cadence units

- `tool_calls` — counted on PostToolUse/PostToolUseFailure, deduplicated
  by `tool_use_id` (a call and its failure count once)
- `prompts` — counted on UserPromptSubmit

## Injection points and the byte cap

gaff only injects on SessionStart, UserPromptSubmit, and PostToolBatch —
events whose output is delivered as session framing. It never decorates
PostToolUse: that context is attached to the tool result and reads as
tool output to the model.

When a threshold crosses on an unsafe event, delivery waits ("armed")
for the next safe one. Per flush, entries merge sections-first then
reminders (config order) then one-shots (by id), separated by blank
lines, under `max_inject_bytes`. An entry that does not fit stays armed
for the next flush and `[gaff:truncated]` is appended.

## State

Session state (counters, armed markers, one-shots) lives outside the
repo under `$XDG_STATE_HOME/gaff/` or `~/.local/state/gaff/`, keyed by
working directory — never inside the repo, where `git clean -xdf` would
erase it mid-session. `GAFF_STATE_DIR` overrides the location.

## Exit codes

Every gaff invocation exits 0 (success) or 1 (internal error) — never 2,
the code that blocks agent sessions. A broken config warns on stderr,
drops a `degraded` marker in the state dir, and gaff continues without
reminders. `gaff doctor` surfaces the degradation.
