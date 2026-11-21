<!-- metadata
title: "CLI reference"
description: "The help gaff prints, for each command that has its own"
type: reference
-->

# CLI reference

`gaff --help` lists every command. Of the commands, only `gaff trust`
prints help of its own. `man gaff` describes each command's options,
and [the configuration reference](configuration.md) describes the
config each one reads.

<!-- Begin generated CLI help: cargo test --test docs -->

## gaff

```console
$ gaff --help
gaff — a context-lifecycle handler for coding agents

Usage: gaff <command> [options]

Commands:
  hook             Handle one hook event from stdin (the harness calls this)
  remind <text> --after N [--id ID]
                   Schedule a one-shot reminder N tool calls ahead
  remind <text> --at stop [--times N] [--id ID]
                   Hold the stop until the hold is cleared, or for N stops
  remind --clear [--id ID]
                   Release one hold, or every hold when no id is given
  status           Print the counters, pending entries, and one-shots as JSON
  init [--host H]  Register the agent hooks in the host's settings file
                   (--uninstall removes them)
  init --git       Write the git hook scripts declared in the config
                   (--uninstall removes them)
  init --github    Generate the workflows declared in the config
  githook <name>   Run one git hook (the installed scripts call this)
  check            Validate the user config and .gaff/gaff.yml together
                   (--handlers checks the user guards and handlers.yml;
                    --github reports a workflow that drifted)
  trust            Allow handlers in this directory (--help says more)
  allow <guard>    Let the next call that guard would refuse through,
                   once
  doctor           Show what is live in this clone
  profile [show]   Show the active profile
  profile list     List the declared profiles and who may set each
  profile set <name>
                   Switch this session to a profile
  run <agent>      Dispatch a hook agent through its runner and map its
                   verdict to an exit code (0 admits, 2 refuses)
  log              Show the injection audit trail for a session
  docs [page]      Print the bundled documentation
                   (search <word>: the pages that hold the word)
  ci [--hook H]    Run the declared git gates against HEAD, as CI does
  reviews          Print the reviews a change must pass, one to a line
  reviews check    Refuse a push whose tips name no review (reads stdin)
  prime            Print what gaff is and how to use it, for an agent's context

Options:
  --session ID     The session for remind, status, allow, profile, and log.
                   Without it, GAFF_SESSION_ID or the host's variable
                   names the session.
  -h, --help       Print this help
  -V, --version    Print the version

A hook exits 0 or 1, so a gaff fault never blocks a session. A guard
refuses a tool call with 2, a hold refuses a stop with 2, run refuses
with 2, and githook returns the failing command's own code.

```

## gaff trust

```console
$ gaff trust --help
gaff trust

Allow handlers in this directory. Consent is per directory:
the record holds one path, and another directory needs its own
grant. It is recorded in ~/.config/gaff/trusted.

To revoke, remove that directory's line from that file.

gaff does not refuse this from an agent. The grant is visible,
but it is not a sandbox: an agent that can write your home
directory can edit the record.

```

<!-- End generated CLI help -->
