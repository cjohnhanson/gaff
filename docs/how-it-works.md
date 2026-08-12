# How it works

## Count, arm, flush

gaff receives every hook event as JSON on stdin and answers on stdout.
Internally each event goes through up to three stages:

1. **Count.** Events that carry a counted unit append one line to a
   per-session append-only ledger. Tool calls dedupe on `tool_use_id`.
2. **Arm.** When a cadence divides the new count, gaff writes a pending
   marker. Nothing is emitted — the crossing usually happens on
   PostToolUse, whose output channel is unsafe to decorate.
3. **Flush.** At the next safe event (SessionStart, UserPromptSubmit,
   PostToolBatch), pending entries are merged under the byte cap and
   emitted as `additionalContext`. Entries are consumed only when
   actually emitted.

## Sessions

Counters are per session (`session_id` from the event payload). A
subagent has its own session and its own counters. `gaff remind` run
inside a session resolves it from `CLAUDE_CODE_SESSION_ID`, which Claude
Code exports to shell subprocesses.

## Compaction

Context compaction erases everything gaff already delivered. On a
SessionStart whose source is `compact`, gaff re-arms consumed one-shots
and re-injects all sections — the ledger survives, cadences continue.

## Failure posture

gaff blocks nothing. Every internal failure degrades to silent
passthrough (exit 0) or a non-blocking error (exit 1). The blocking
exit code 2 is never emitted — enforced by tests. Degradation is loud:
stderr warnings, a `degraded` state marker, and `gaff doctor`.
