<!-- metadata
title: gaff CLI reference
description: Every command and option of the gaff binary, generated from its man page
type: reference
-->

# gaff CLI reference

## NAME

`gaff` — context-lifecycle handler for coding agents

## SYNOPSIS

`gaff` `command` \[`options`\]

## DESCRIPTION

`gaff` manages session context for a coding agent through the harness hook system: counters, cadence reminders, one-shot reminders, and prime sections. A hook exits 0 or 1, so a gaff fault never blocks a session. A guard refuses a tool call with 2, a hold or a blocking handler refuses a stop with 2, `run` refuses with 2, and `githook` returns the failing command's own code.

## COMMANDS

`hook`  
Handle one hook event from stdin. The harness calls this; it is not for interactive use.

`remind` `text` (`--after` `n` \| `--at` `stop`) \[`--times` `n`\]  
Schedule a one-shot reminder. With `--after` `n` it fires `n` tool calls ahead. With `--at` `stop` it fires at the stop and holds the stop open until it is cleared. `--times` `n` lets the hold go on its own after `n` refused stops; it applies to `--at` `stop` only. `--id` `id` names the one-shot; scheduling the same id again re-arms it. `--session` `id` selects the session; the default comes from the environment.

`remind` `--clear` `--id` `id`  
Clear the one-shot with that id, and release the hold it carries.

`status`  
Show the counters, the pending entries, and the one-shots. `--session` `id` selects the session.

`init` \[`--host` `name`\] \[`--uninstall`\]  
Register the agent hooks in the host's settings file. Two hosts are implemented, Claude Code and `generic`; Claude Code is the default.

`init` `--git` \[`--uninstall`\]  
Write one script in .git/hooks for each git entry the config declares. gaff never sets `core.hooksPath`, because that setting names one directory and would disable every hook another tool installed. A hook gaff did not write is kept as \<hook\>.local and called first.

`init` `--github`  
Render each declared workflow to .github/workflows. gaff cannot run a GitHub event, so this domain is generated and checked, never executed.

`githook` `name` \[`args`\]  
Run one git hook. The installed scripts call this. It returns the failing command's exit code, because a git hook exists to block the commit or the push.

`check` \[`--handlers`\]  
Validate .gaff/gaff.yml and report every problem. With `--handlers`, validate ~/.config/gaff/handlers.yml instead. With `--github`, report a workflow that drifted from the config. It exits 1 on a drift, so CI can run it. The plain form stays repo-only, so it behaves the same in CI.

`doctor`  
Show what is live in this clone: config, hooks, and state.

`profile` \[`show` \| `list` \| `set` `name`\] \[`--session` `id`\]  
Show, list, or set the active profile. A profile is a named overlay: it selects which reminders and sections are active and may override their cadences. The resolution path is the environment `GAFF_PROFILE`, then the session state, then .gaff/profile, then the config default. A switch re-primes the sections on the next flush.

Profiles are advisory. gaff blocks nothing, and an agent that can write files can edit the config regardless. The transition policy refuses the agent-facing path: a profile absent from `transitions.agent_may_set` is human-only. gaff decides who is asking by structural identity, where a terminal on stdin is a human and anything else is an agent.

`run` `agent`  
Dispatch a hook agent through its runner and map the agent's verdict to an exit code: 0 admits, 2 refuses. gaff runs the agent's context command, feeds its output to the runner on standard input, and reads a `gaff-verdict: pass` or `gaff-verdict: fail` line. The runner is a config choice; the default is `kersh`, a separate program. Every failure refuses: a fail verdict, a missing verdict, a non-zero runner, a timeout, a failed context, an unknown agent, and an untrusted directory all exit 2.

`trust`  
Allow handlers to run in this directory. Every agent tool call passes through `gaff hook` first, and a built-in guard there refuses this command. The guard matches the command as text, so a form it does not match reaches the shell unrefused. A quoted subcommand is one such form. The user's shell has no hook, so it runs there. An agent that can write the home directory can still edit the file, so this raises the cost and makes the grant visible; it is not a sandbox. A handler's command inherits the working directory gaff was called in, and tools such as git(1), make(1), and just read executable settings from there. So handlers are refused by default, and consent is per directory. The record holds the working directory this command ran in, and a handler runs only where that path matches. Another directory needs its own `trust`. Consent is recorded in ~/.config/gaff/trusted, outside every repo tree.

`log` \[`--session` `id`\]  
Print the injection audit trail: every flush gaff delivered to the session, with the byte count and the entry names.

`allow` `guard`  
Let the next tool call that `guard` would refuse through, once. The built-in guard in `gaff hook` refuses this command from an agent; run it from your shell.

`ci` \[`--hook` `name`\]  
Run the declared git gates against HEAD, as CI does: the config must load, no declared workflow may drift, then each hook's entries run, pre-commit then pre-push, with a synthesized push on stdin for the latter. A requested hook with no entry fails the run.

`reviews` \[`check`\]  
With no argument, print the reviews a change must pass, one to a line, in declaration order. A merge gate reads the list from here rather than from the config file, so a format change cannot break the gate silently. Where the repository config holds no `reviews` key, the command exits 1 and names the fix. An absent policy must not read as "no review is required". Where that config holds `reviews: []`, the command exits 0 and prints the user config's names, if it declares any. The user config and the repository config both contribute names, and neither drops a name from the other. A user config cannot state the policy for a repository that omits one.

`reviews check` reads git's pre-push ref lines on standard input. Every declared review needs one sign-off on every pushed tip. The check refuses a push whose tip lacks one. The command takes no argument.

A sign-off is one line of the tip's `reviews` git note, reading `signoff[NAME] VERDICT SHA EVIDENCE`. The verdict is `PASS` or `FAIL`. The check ignores prose around the lines, so a note can carry a narrative. The sha names the commit the reviewer read. A sign-off therefore cannot carry forward onto a commit nobody read.

The check refuses a push on any of these faults:

- a declared review with no sign-off

- a verdict of `FAIL`

- a sign-off naming another commit

- two sign-offs for one review

- evidence of fewer than 3 words

- a sha of fewer than 7 characters, or one that is not hex, or one longer than 64

- no note at all

The command also refuses a push where a line on standard input carries fewer than the four fields git writes for a ref. That is a truncated stream rather than a review fault. Standard input carrying no line at all is neither: git runs the hook that way when the remote already holds everything, and a push that moves no ref lands no commit.

The floor on the sha is what makes a prefix mean anything. One hex character matches one commit in sixteen, so a sign-off reading `PASS 5` would name every commit whose sha starts with `5`.

A pull request needs the branch head, because the commit CI checks out is a merge commit no reviewer saw. `ci` substitutes that sha into the ref line it synthesizes on the hook's standard input. So one mechanism carries the head, and this command takes no flag that names a commit.

`docs` \[`page`\]  
Print the bundled documentation. Without `page`, list the available pages.

`prime`  
Print what gaff is and how to use it, for an agent's context. The output depends only on the binary version: no arguments, config, or state changes it. Put it into an agent's context; policy about when to use gaff belongs to the caller.

## ENVIRONMENT

`GAFF_STATE_DIR`  
The state root. A relative path joins the working directory. Without it, the root falls back to a user-scoped path. The root is never inside the repo.

`GAFF_PROFILE`  
The active profile. It outranks the session state and .gaff/profile.

`GAFF_HANDLERS`  
Set it to `off` to disable every handler. Use it when a handler wedges a session.

`GAFF_HOST`  
The host adapter for `gaff hook`. Without it, gaff selects the adapter from the payload shape.

## FILES

.gaff/gaff.yml  
The repo config: reminders, sections, profiles, and cadences. It never holds a command.

~/.config/gaff/gaff.yml  
The same keys, for every repo. A repo entry under a name the user declared is refused with a warning, and the user's entry stands. The user value of `transitions` wins, because a repo must not widen what an agent grants itself. Both files contribute names to `reviews`, and neither drops a name from the other.

~/.config/gaff/handlers.yml  
The handlers. Only this file may declare a command.

~/.config/gaff/trusted  
The directories that may run handlers.

## EVENTS

gaff names an event for what it means, not for what a host calls it. An adapter maps its host's names onto this set:

`session_start`  
A session begins or resumes. A flush point.

`prompt`  
The user submits a prompt. A flush point.

`tool_call`  
One tool call finished. gaff counts these.

`tool_batch`  
A batch of tool calls finished. A flush point.

`stop`  
The agent finished a turn. A flush point.

A host event outside this set stays first-class. gaff forwards it and permits nothing.

## HANDLERS

A handler is an external command whose stdout becomes injected context. Handlers are declared only in ~/.config/gaff/handlers.yml. A repo cannot declare one, and gaff does not consult `GAFF_CONFIG_DIR` or `XDG_CONFIG_HOME` on the hook path, because a repo can set an environment variable through direnv, mise, or a committed settings file.

The first element of `command` must be an absolute path. gaff never searches `PATH`, because a repo can prepend to it. The child's environment is an allowlist, not a denylist, so it receives no token unless a handler names one in `env_passthrough`. Handlers run in sequence inside one deadline for each flush. Run `gaff` `docs configuration` for the full reference.

## EXIT STATUS

A gaff failure exits 0 or 1, never 2. Exit 2 is the agent host's blocking code, and no gaff failure may block a session.

A refusal is not a failure, and a refusal exits 2 on purpose. A guard refuses a tool call, a hold or a blocking handler refuses a stop, and `run` reports an agent's refusal.

`githook` returns the exit code of the command that failed, because a git hook exists to block.

## SEE ALSO

missouri(1), tisket(1)
