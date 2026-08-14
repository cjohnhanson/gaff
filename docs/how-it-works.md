# How it works

## Count, arm, flush

gaff reads every hook event as JSON on stdin and answers on stdout. Each
event goes through up to three stages:

1. **Count.** An event that carries a counted unit appends one line to
   the per-session append-only ledger. Tool calls deduplicate on
   `tool_use_id`.
2. **Arm.** When a cadence divides the new count, gaff writes a pending
   marker. gaff emits nothing here. The crossing usually happens on
   PostToolUse, whose output channel is unsafe to decorate.
3. **Flush.** The safe events are SessionStart, UserPromptSubmit, and
   PostToolBatch. At the next one, gaff merges the pending entries under
   the byte cap. It emits them as `additionalContext`. gaff consumes an entry only when it
   emits the entry.

## Sessions

The counters are per session. gaff reads the `session_id` from the event
payload. A subagent has its own session and its own counters. `gaff
remind` runs inside a session and resolves the session from
`CLAUDE_CODE_SESSION_ID`, which Claude Code exports to a shell
subprocess.

## Compaction

A context compaction erases everything that gaff already delivered. On a
SessionStart whose source is `compact`, gaff re-arms the consumed
one-shots and injects every section again. The ledger survives, and the
cadences continue.

## Failure posture

gaff blocks nothing. Every internal failure degrades to a silent
passthrough at exit 0, or to a non-blocking error at exit 1. gaff never
emits the blocking exit code 2, and the tests enforce that. The
degradation is loud: a warning on stderr, a `degraded` state marker, and
the `gaff doctor` report.
