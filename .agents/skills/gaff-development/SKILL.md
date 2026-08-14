---
name: gaff-development
description: Conventions for developing gaff itself — the never-exit-2 rule, the injection-point rule, the repo-config-is-data rule, the adapter seam, and how to test with missouri. Use when you change gaff's source, add a command, add a config key, or add support for another agent host.
---

# Developing gaff

Four invariants govern this codebase. Each exists because breaking it
damages a live agent session, and each is enforced by a test.

## 1. gaff never exits 2

Exit 2 is the agent host's blocking code. A gaff failure must never
block a session. A config typo, an unwritable state directory, a bad
flag, and a dead handler all exit 0 or 1 and print a warning.

This is why the CLI parses arguments by hand instead of using clap:
clap exits 2 on a usage error. `cli::tests::no_usage_error_ever_exits_two`
covers it. Add every new usage error to that list.

## 2. Injection happens only at flush points

`SessionStart`, `UserPromptSubmit`, and `PostToolBatch` are the only
events whose context reaches the model's session framing. `PostToolUse`
context rides the tool result instead, so text injected there is not
session framing.

Use `engine::is_flush_event` rather than a second list. A handler that
subscribes to a non-flush event is a config error for this reason.

## 3. The repo config is data; executables are user-scoped

`.gaff/gaff.yml` declares sections, reminders, cadences, and profiles.
It never names a command. Handlers live only in
`~/.config/gaff/handlers.yml`, and gaff does not read `GAFF_CONFIG_DIR`
or `XDG_CONFIG_HOME` on the hook path, because a repo can set an
environment variable through direnv, mise, or a committed settings file.

A handler's child still runs with the repo as its working directory,
and tools such as git, make, and just read executable settings from
there. That is why handlers are deny-by-default per repo. Never weaken
`handler::is_trusted` for convenience.

## 4. Host knowledge lives behind the adapter seam

`src/adapter.rs` is the only module that knows a host's payload shape,
event names, or settings path. Everything above it uses the normalized
`Envelope`.

To add a host, add an `Adapter` constant with that host's real field
names taken from its documentation, and add it to `ADAPTERS`. Do not
guess a schema: a guessed schema fails at run time inside a user's
session, which is worse than no support.

## Structure

The CLI lives in `src/cli.rs` inside the library, and `src/main.rs` is a
shim. That keeps the whole command surface testable in process and
matches the other tools in this ecosystem (tisket, zettel, almanac,
belmont).

## Testing

Unit tests cover the internals. The missouri suite in `tests/missouri/`
covers the CLI end to end as real subprocess invocations, and it is the
primary evidence that a command behaves. Run both:

    cargo test
    cargo clippy --all-targets     # must be silent
    cd tests/missouri && missouri run

When you add a command or a config key, add a missouri state pair for
it. A state is a directory; a transition is a shell command; the
comparison is a byte diff of the resulting tree, so the fixture records
the real state. Prefer asserting on structural state (a consumed cursor,
a remaining pending marker) over asserting on output text.

Machine-specific or timing-coupled bytes belong in `.missouri/ignore`,
not in a fixture.

## Prose

All prose, including code comments, follows ASD-STE100: short
declarative sentences, active voice, one topic per sentence, and no
idioms.
