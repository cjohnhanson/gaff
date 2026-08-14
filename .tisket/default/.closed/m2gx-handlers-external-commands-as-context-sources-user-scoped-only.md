---
title: 'handlers: external commands as context sources, user-scoped only'
status: done
priority: null
assignee: null
due_date: null
labels:
- feature
depends_on: []
created: '2026-08-14T20:34:26Z'
updated: '2026-08-14T21:17:05Z'
---

## Scratch Notes

## Review Log
- round 1 (plan): security + concurrency, 1 subagent, 5B / 6M / 8m
  Outcome: v1's central claim was wrong. v1 argued that user-scoped-only
  config removed the need for a per-repo trust gate. Four findings broke
  that: the config path was env-selectable, the child's cwd is the
  hostile repo (git/make/just/npm all read executable config from cwd),
  argv[0] resolved through a repo-influenced PATH, and handler stdout
  wore gaff's own attribution prefix. The trust gate is reinstated.
  MINOR 19 was a live pre-existing bug, verified and shipped separately.

# Plan v2: handlers — external commands as context sources

## Why
The build plan listed handlers and nothing was built. gaff injects only
static text and static files. A handler makes injected context derived.

## Threat model, stated plainly
gaff runs with the agent's full privileges, and the child's working
directory is the repo under test, which may be hostile. Three boundaries
must hold, and v1 held none of them:

1. A repo must not choose WHICH handlers exist. -> config is read from
   `$HOME/.config/gaff/handlers.yml` and nothing else. No env override
   on the hook path: `GAFF_CONFIG_DIR` and `XDG_CONFIG_HOME` are not
   consulted, because direnv, mise, and a committed devcontainer or
   settings env block all let a repo set them. Tests set `HOME`.
2. A repo must not choose WHAT a handler's command resolves to.
   -> `command[0]` must be an absolute path. gaff does not search PATH.
   This is stricter and simpler than sanitizing PATH, and it removes the
   `bin/gh` shadowing class outright.
3. A repo must not have its content EXECUTE just because a handler ran
   in it. This one cannot be fully closed: `git status` honors
   `core.pager` from `.git/config`. So gaff defaults to deny and needs
   explicit per-repo consent.

### The trust gate (reinstated)
Handlers run only when the repo's canonical path is listed in
`$HOME/.config/gaff/trusted`, one absolute path per line. `gaff trust`
adds the current repo, and only from a terminal (the same structural
identity rule `gaff profile set` already uses). Default deny, silent.
`GAFF_HANDLERS=off` is a first-line kill switch for a wedged session.

## Execution model
Sequential, not parallel. The reviewer's scope cut, accepted: with a
global deadline and a cadence, parallelism buys nothing and deletes the
scoped-thread failure class.

- Each child: `process_group(0)`, stdout and stderr both piped.
- One detached reader thread per child, result over an `mpsc` channel,
  collected with `recv_timeout`. A scoped thread cannot time out,
  because the thread that would notice the deadline is the one blocked
  on the pipe.
- On deadline: `SIGKILL` the negated process group, not the pid. A
  grandchild inherits the stdout write end, so killing only the child
  leaves the pipe open and the hook never returns. That hangs the whole
  session, which is worse than any injection failure.
- Read cap 64 KiB, then kill. Bounds memory on the hot path.
- Always `wait()` to reap.
- Budgets: one wall-clock deadline shared by all handlers per event.
  `SessionStart` 2000 ms, every other flush point 500 ms. Per-handler
  default 300 ms, `timeout_ms` capped at 2000.

## Cadence
A handler carries the same `Every` cadence as a reminder and arms
through the existing `arm_crossings`. A handler runs only when its
cadence crosses, so a `PostToolBatch` handler does not spawn a process
on every batch. This reuses the pending/consume machinery and replaces
the reviewer's proposed cooldown-plus-cache with existing code.

## Config
`$HOME/.config/gaff/handlers.yml`:

```yaml
handlers:
  - name: ci
    events: [SessionStart, PostToolBatch]
    every: {tool_calls: 20}
    command: ["/opt/homebrew/bin/gh", "run", "list", "--limit", "1"]
    timeout_ms: 300
    max_bytes: 1024
    when:
      file_exists: ".github/workflows"
      branch_prefix: "feat/"
      env: {CI: "1"}
      cwd_prefix: "/Users/me/Projects"
```

- `events` must be flush points; reuse `engine::is_flush_event` so the
  lists cannot drift. A non-flush subscription is a config error.
- `command` is argv, never a shell string. `["/bin/sh","-lc",...]` is an
  explicit user choice.
- `when` predicates all must pass. `branch_prefix` reads `.git/HEAD`
  directly and follows a `gitdir:` file for worktrees; it never shells
  out to git, which would re-enter the `.git/config` execution problem.

## Output handling
- Prefix `[gaff:handler:<name>]`. The `handler:` infix means handler
  output can never be confused with a section or a reminder, in the
  injected text or in `injections.jsonl`. It also resolves the
  cross-file name collision, since `gaff check` cannot see both files.
- Strip any output line matching `^\s*\[gaff:`. Commit messages and
  branch names are attacker-controlled and would otherwise forge an
  entry in the model's session framing and in `gaff log`.
- `String::from_utf8_lossy`; arbitrary output is not valid UTF-8.
- stderr is captured, truncated to 200 bytes for the warning, never
  injected.
- Truncate an oversized entry rather than drop it, and mark it. A
  handler entry is `Unconditional` and has no pending marker, so a drop
  loses the output where a reminder would retry.
- Merge order: sections, reminders, one-shots, THEN handlers. Derived
  context yields to scheduled context.

## Environment
Strip before spawn: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `DYLD_*`,
`BASH_ENV`, `ENV`, `NODE_OPTIONS`, `PYTHONSTARTUP`, `PYTHONPATH`,
`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_SSH_COMMAND`,
`GIT_EXTERNAL_DIFF`, `GIT_PAGER`. Export `GAFF_EVENT`,
`GAFF_SESSION_ID`, `GAFF_HANDLER_NAME`, `GAFF_TIMEOUT_MS`. No payload
passthrough: it carries the user's prompt text, which a network-capable
handler would exfiltrate, and it is host-specific, which would undo the
adapter seam.

## Working directory
`Command::current_dir(std::env::current_dir())` explicitly, the same
source `config::load` uses. Never `envelope.cwd`; the two diverge for
worktrees and subagents, and reading one repo's config while running in
another is a silent boundary crossing.

## Docs must stop overclaiming
"Not a way to run repo code" becomes "no repo-declared handler". State
that a handler's command runs with the repo as its working directory and
that many tools read executable settings from there. Drop "a repo cannot
trigger a handler": `file_exists` and `branch_prefix` mean repo content
decides whether a handler fires. That is by design, not a security
property.

## Rejected from the review, with reasons
- Keeping `GAFF_CONFIG_DIR` gated on `is_terminal`: rejected. Tests set
  `HOME` instead, so the variable does not need to exist at all. Fewer
  paths, nothing to gate.
- Sanitizing PATH: rejected in favor of requiring an absolute
  `command[0]`. Same protection, far less machinery.
- Cooldown plus stale-output cache: rejected in favor of the existing
  cadence machinery, which already solves the spawn-rate problem.

## Files
- `src/handler.rs` (new): config, predicates, trust, spawn, sanitize.
- `src/config.rs`: name a `handlers:` key in the repo config as an error
  that points at the user-scoped path, without discarding the rest.
- `src/engine.rs`: run armed handlers at a flush; merge last.
- `src/main.rs`: `gaff trust`; `gaff check --handlers`; doctor lists
  handlers, their resolved absolute command, and the trust state.
- `docs/configuration.md`, `docs/man/gaff.1`, `README.md`.
- `tests/missouri/handled*`: predicate pass, predicate fail, non-zero
  exit, untrusted repo, and a timeout using a 10x sleep with no elapsed
  value in the asserted text.

## Review Log (final)
- round 2 (QA, exercised): 1 subagent, 1B / 6M / 6m
  BLOCKER: child.wait() unbounded — a child that closed stdout and kept
  running held the hook open for its whole life (60s/20s/15s reproduced).
  Majors: GIT_CONFIG_COUNT bypassed the env denylist and executed a repo
  script; a failed handler never spent its cadence and respawned every
  flush; the [gaff: defusing missed zero-width and ANSI prefixes;
  over-cap output was discarded twice over; SessionStart handlers could
  never fire on a fresh session; the trust doc overclaimed.
  All fixed and verified against the reproductions. Shipped.
- Layout brought to the ecosystem pattern: CLI moved to src/cli.rs in the
  lib (main.rs is a 12-line shim), src/error.rs added, first CLI tests
  assert the never-exit-2 rule. .agents/skills/gaff-development added.
  Consumer skill updated for profiles, handlers, trust, and log.
