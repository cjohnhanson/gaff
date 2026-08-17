# Configuration reference

The repo config lives at `.gaff/gaff.yml`. It holds data only: text, file
paths, and cadences. gaff executes nothing in it.

## Where a config lives

gaff reads two data configs and lays one over the other.

| File | Scope | Holds |
|------|-------|-------|
| `$HOME/.config/gaff/gaff.yml` | every repo | The keys below |
| `.gaff/gaff.yml` | this repo | The keys below |
| `$HOME/.config/gaff/handlers.yml` | every repo | Handlers only |
| `$HOME/.config/gaff/trusted` | every repo | The repos that may run handlers |

A person works in many repos and wants some reminders everywhere. Put
those in the user config. A repo adds to what the user declared, and it
never overrides it.

The rule is one sentence: **a repo may add, and it may not take a name
the user already used.** A clone is untrusted content. If a repo could
take a user entry's name, it would decide what that entry says while
keeping the user's label on it.

- `reminders`, `sections`, `profiles`, `git`, and `github` merge by
  name. A repo entry under a name the user declared is refused with a
  warning, and the user's entry stands. Every other entry from both
  files stays active.
- `guards` are user-only. A repo that declares one is warned about and
  ignored, because a guard is the only thing that blocks.
- A repo section resolves its `file` against `.gaff/`. A user section
  resolves against `$HOME/.config/gaff/`. Each reads only from its own
  directory.
- `max_inject_bytes` takes the repo value, but never below the user's
  when the user declared any entry. A one-byte cap would silence them
  all.
- `default_profile` takes the repo value, unless it names a profile the
  user declared. The same holds for a committed `.gaff/profile`. A repo
  may select its own profiles, not the user's.
- A repo profile filters and retimes the repo's own entries. Its `only`,
  `disable`, `cadence`, and `max_inject_bytes` do not reach a user
  entry.
- `transitions`: the user value wins whenever the user sets one. That
  field says which profiles an agent may grant itself, and a repo must
  never widen it.
- `reviews`: both lists join, and neither drops a name from the other.
  A script reads this list to decide whether a change may merge, so a
  layer that replaced it could require nothing. The repo must state the
  policy: a user config adds a name and never supplies a list the repo
  omitted.

Only the user config may hold handlers, and they live in their own file.
That keeps the security boundary easy to check: a command can only come
from `handlers.yml`.

## Top-level fields

| Field | Default | Description |
|-------|---------|-------------|
| `sections` | `[]` | The prime sections (see below) |
| `reminders` | `[]` | The recurring reminders (see below) |
| `max_inject_bytes` | `4096` | The hard cap per flush, truncation marker included |
| `profiles` | `{}` | The named overlays (see below) |
| `default_profile` | none | The profile that applies when nothing else selects one |
| `transitions` | `{}` | Which profiles an agent may select for itself |
| `git` | `[]` | The git-hook entries (see below) |
| `github` | `[]` | The workflows to generate (see below) |
| `guards` | `[]` | The tool calls to refuse (user config only) |
| `reviews` | none | The reviews a change must pass (see below) |

## Reviews

A repository names the independent reviews a change must pass:

```yaml
reviews:
  - review-tests
  - review-docs
```

Each name is a review skill the repository carries. gaff records the
policy and enforces nothing. A separate script reads the list and
decides whether a change may merge.

`gaff reviews` prints one name to a line, in declaration order. A
script that read this file itself would break when the format changes,
and it would then require nothing.

An absent `reviews:` key and `reviews: []` mean different things:

| The repository config | `gaff reviews` | The meaning |
|-----------------------|----------------|-------------|
| no `reviews:` key | exits 1, names the fix | Nobody stated a policy |
| `reviews: []` | exits 0, prints the user's names if any | An author requires none of its own |
| one or more names | exits 0, prints them | Those reviews are required |

The error matters. A deleted `reviews:` key must not mean "no review is
required", because a script would then merge an unreviewed change. An
author who wants no review writes `reviews: []`.

A missing config file, an empty file, and a file gaff cannot parse each
exit 1.

The repository states the policy, and a user config adds to it. Where
the repository states one, both lists join and neither drops a name
from the other. The user's names come first, then the repository's.

Where the repository states none, the command exits 1 and the user's
list does not stand in. That holds for a repository config that is
missing, empty, unparseable, or without the key. Gate policy belongs
to the repository. A truncated config beside a user config would
otherwise read as a policy the repository never wrote.

`gaff check` refuses an empty name, a name holding whitespace, and a
name repeated within one config. A name holding a newline would become
two requirements, because a caller reads one name to a line. A name in
both configs is not a repeat; the merge keeps one copy.

`gaff reviews` does not run these checks. A caller that needs them runs
`gaff check` too.

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

gaff injects on SessionStart, UserPromptSubmit, PostToolBatch, and Stop.
The harness delivers the output of those events as session framing. gaff
never decorates PostToolUse. That context attaches to the tool result,
and the model reads it as tool output.

Stop is the last moment before the model walks away, which makes it the
one point where "is this actually done" can still change the answer.
Every rule of the form *drive the work to done* or *check the gate
before saying shipped* applies exactly there.

A threshold that crosses on an unsafe event arms the entry. The delivery
waits for the next safe event. Per flush, gaff merges the sections first,
then reminders in config order, then one-shots by id. A blank
line separates each entry, and the total stays under `max_inject_bytes`.
An entry that does not fit stays armed for the next flush, and gaff
appends `[gaff:truncated]`.


## Refusing a stop

A guard refuses a tool call. A **stop hook** refuses the stop itself.
Neither is a fault, so the exit-code rule holds: gaff's own failures
still exit 0 or 1, and 2 belongs to the places that mean it.

There are two kinds, and the difference is who may run a command.

### A blocking handler: the condition is a command

Set `blocks: true` on a handler that subscribes to `stop`. A non-zero
exit refuses the stop, and the command's output is the message.

```yaml
handlers:
  - name: tests-pass
    events: [stop]
    blocks: true
    command: ["/opt/homebrew/bin/just", "test"]
```

A blocking handler takes no `every`: it is a gate, and a gate that only
sometimes gates is not one. It runs at every stop. Only `stop` accepts
`blocks`, because every other flush point is a moment that has already
happened, so there is nothing left to refuse.

This lives in the user config, which is why the command may run at all.
A repo cannot declare a handler.

### A hold: the condition is text the model judges

An agent sets this for itself, mid-session:

```
gaff remind "The goal is X, and it is not reached yet." --at stop --id goal
gaff remind --clear --id goal
```

gaff refuses the stop and delivers the text. The model reads it and
decides whether the work is done, then releases the hold. Nothing runs,
which is why an agent may set one: `gaff trust` exists precisely so an
agent cannot schedule command execution for itself, and a hold needs no
such right.

A session may hold several times under different ids. The first one
still held refuses the stop.

`--times N` shapes the hold: it refuses N stops and lets the next one
through on its own. That is "push back this many times, then let me
stop", as against a hard hold that lasts until cleared.

```
gaff remind "Are you sure every reviewer converged?" --at stop --times 2 --id sure
```

### Neither can wedge a session

A stop hook that can never be satisfied would end a session's ability to
end, and nothing inside the session could undo it. So gaff counts
consecutive refusals and lets the stop through after twelve, saying so.
A condition that cannot start, or that outruns its timeout, allows the
stop rather than refusing on a gate that did not run.

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
session. That is the question a reader has when a reminder seems to
fire at the wrong time.

## Handlers

A handler is an external command whose stdout becomes injected context.
Handlers make injected context derived rather than static.

Handlers live **only** in `$HOME/.config/gaff/handlers.yml`. A repo
cannot declare one. gaff does not read `GAFF_CONFIG_DIR` or
`XDG_CONFIG_HOME` on the hook path. A repo can set an environment
variable through direnv, mise, a devcontainer, or a committed settings
file. An env-selectable config path is therefore a repo-selectable
command.

```yaml
handlers:
  - name: ci
    events: [session_start, tool_batch]
    every: {tool_calls: 20}   # a SessionStart run ignores the cadence
    command: ["/opt/homebrew/bin/gh", "run", "list", "--limit", "1"]
    timeout_ms: 300
    max_bytes: 1024
    when:
      file_exists: ".github/workflows"
      branch_prefix: "feat/"
```

| Field | Default | Description |
|-------|---------|-------------|
| `name` | required | The entry name; it appears as `[gaff:handler:<name>]` |
| `events` | required | Flush points only |
| `command` | required | The argv. `command[0]` must be an absolute path |
| `every` | required | The cadence, as for a reminder |
| `timeout_ms` | 300 | The per-handler deadline, capped at 2000 |
| `max_bytes` | 1024 | The injected size; the output truncates rather than vanishing |
| `when` | none | Predicates; every declared predicate must pass |

`when` accepts `file_exists`, `cwd_prefix`, `branch_prefix`, and `env`.
`branch_prefix` reads `.git/HEAD` directly and follows a worktree's
`gitdir:` file. It never runs `git`, because that would honor the
repo's own `.git/config`.

### What runs, and what that costs you

**A handler's command runs with the repo as its working directory.**
Many ordinary tools read executable settings from there. `git` honors
`core.pager` and `core.fsmonitor` from `.git/config`. `make`, `just`,
and `npm` read their own repo files. gaff cannot close that. Handlers
are therefore **deny-by-default**:

```
gaff trust          # from a terminal, in the repo you want to allow
```

Consent is recorded in `$HOME/.config/gaff/trusted`, outside every repo
tree, and that file must not be writable by other users. Without
consent, no handler runs and gaff says so once.

Note the limit of the gate: `gaff trust` refuses a caller whose stdin
is not a terminal, so an agent cannot grant consent *through gaff*. An
agent that can write your home directory can still edit the file. The
gate raises the cost and makes the grant visible. It is not a sandbox.

`command[0]` must be an absolute path. gaff never searches `PATH`,
because a repo can prepend its own `bin/` and shadow the binary you
named.

The child's environment is an **allowlist**, not a denylist. The child
gets `HOME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TERM`, `TZ`, `USER`, a
sanitized `PATH`, and the `GAFF_*` variables below. It gets nothing
else, and that includes your API tokens.

A denylist kept losing: stripping `GIT_CONFIG_GLOBAL` still leaves
`GIT_CONFIG_COUNT`, which does the same job, and every runtime adds
another loader variable. A handler that genuinely needs a secret names
it:

```yaml
    env_passthrough: [GITHUB_TOKEN]
```

The grant is then explicit and visible in the config.

gaff filters `PATH` to absolute entries outside the repo. The child
resolves its own helpers by name. git calls `ssh`, and a script uses
`#!/usr/bin/env`. A repo entry on `PATH` would shadow them.

gaff exports `GAFF_EVENT`, `GAFF_SESSION_ID`, `GAFF_HANDLER_NAME`, and
`GAFF_TIMEOUT_MS`. It never passes the hook payload, which holds the
user's prompt text.

Handler output is untrusted: commit messages and branch names reach it
from a cloned repo. gaff drops the characters that render as nothing. These are the control
codes, the zero-width characters, and the format characters. gaff then
defuses the token `[gaff:` anywhere on a line. Output cannot pose as a
section or a reminder, in the session framing or in `gaff log`.

Output larger than 64 KiB is cut at that point rather than discarded.
Output that does not fit the flush's byte cap is truncated with a
marker. A handler's cadence is already spent, so a drop would lose the
output for good.

`GAFF_HANDLERS=off` disables every handler. It is the switch to reach
for when a handler wedges a session.

### Cost

Handlers run in sequence inside one shared deadline per flush: 2000 ms
at `SessionStart`, 500 ms at every other flush point. A handler that
misses the budget is skipped. A handler that overruns its deadline is
killed, along with its process group.

Two separate things are bounded, and both must be: the read, and the
child. A grandchild that inherits the output pipe holds the read open. A child
that closes its output and keeps running holds the wait open. Either
one hangs the session. gaff bounds both, and it kills the process
group.

A cadence counts tool calls and prompts, and a fresh session has
neither, so a `SessionStart` subscription runs at session start
regardless of `every`. Every other flush point waits for a crossing.

`gaff check --handlers` validates the user config and exits 1 on a
problem, including a config it cannot parse. It covers handlers and
guards, which both live only in that layer. `gaff doctor` lists the
declared handlers and whether this repo is trusted.

Plain `gaff check` validates the **effective** config, which is the
user layer with the repo layer over it. That is what a hook will
actually run, so it is what the check reports. A machine with a user
config can therefore see a problem that CI does not, and CI can see one
that a workstation does not. Run `gaff check --handlers` to isolate the
user layer.

## Guards

A guard refuses a tool call before it runs. Declare it once in the
user-level config and it applies in every repo.

```yaml
guards:
  - name: no-mass-stage
    tool: Bash
    matches: 'git((?:[ \t]|\\\r?\n)+-[^\s"'';&|()<>]+((?:[ \t]|\\\r?\n)+[^\s"'';&|()<>]+)?)*(?:[ \t]|\\\r?\n)+(add|stage)((?:[ \t]|\\\r?\n)+(?:"[^"]*"|''[^'']*''|[^\s"'';&|()<>]+))*?(?:[ \t]|\\\r?\n)+["'']?(-[A-Za-z]*A[A-Za-z]*|--all|\.\.?/*\*?|:/\.?|:\(top\)|\*)["'']?($|[^A-Za-z0-9_/.-])'
    unless: '--dry-run'
    message: >-
      Stage files by name. Run `git status` first, then name each path.

  - name: no-commit-all
    tool: Bash
    matches: 'git((?:[ \t]|\\\r?\n)+-[^\s"'';&|()<>]+((?:[ \t]|\\\r?\n)+[^\s"'';&|()<>]+)?)*(?:[ \t]|\\\r?\n)+commit((?:[ \t]|\\\r?\n)+(?:"[^"]*"|''[^'']*''|[^\s"'';&|()<>]+))*?(?:[ \t]|\\\r?\n)+(--all|-[A-Za-z]*a[A-Za-z]*)($|[^A-Za-z0-9_-])'
    message: >-
      `git commit -a` stages every tracked change. Stage the paths you
      mean, then commit without -a.

  - name: no-credential-reads
    tool: Read
    field: file_path
    matches: '(\.env$|/\.ssh/|id_rsa|\.pem$)'
    message: That path holds credentials. Do not read it.
```

| Field | Default | Description |
|-------|---------|-------------|
| `name` | required | The guard name, printed on a refusal |
| `tool` | required | The tool to inspect, such as `Bash` or `Read` |
| `matches` | every call | A regular expression |
| `field` | `command` | The tool-input field to match; use `file_path` for a file tool |
| `unless` | none | A pattern that exempts a call `matches` caught |
| `message` | required | What the agent reads. Say what to do instead |

The pattern is a regular expression, not a glob, so it can express what
a glob cannot. `git\s+add\s+.*-A` catches a compound command such as
`cd somewhere && git add -A`. A glob anchored to the start of the line
does not, and that exact gap let a private key reach a public repo.

### Letting one call through: `gaff allow`

A guard is config, and nothing at runtime lifts one. That is deliberate:
an agent that could lift a guard would lift the mass-stage guard. But
the human sometimes wants one specific call to go through, without
editing the file. That is `gaff allow`:

```
!gaff allow no-mass-stage
```

The next call that guard would refuse passes instead, once, with a note
on stderr saying so. The call after that is refused again.

Two things stand between an agent and its own grant. `gaff allow`
refuses to run without a terminal on stdin, the same check `gaff trust`
makes. And gaff carries a built-in guard, which no config declares and
no config removes, that refuses `gaff allow` and `gaff trust` from any
Bash call an agent makes. The boundary is structural: every agent
command passes through `gaff hook` first, and a human's shell has no
hook. `!gaff allow` in the harness runs in the human's shell.

Note the limit, which is the one `gaff trust` carries. `gaff allow`
records the grant in gaff's state directory. An agent that can write
that directory can arm the one-shot without running the command, and an
agent that can allocate a terminal passes the stdin check. The gate
raises the cost and makes the grant visible. It is not a sandbox.

The `!` prefix runs the command in the harness's own shell. That is what
puts a terminal on stdin and keeps the call out of the hook.

### The one place gaff exits 2

Everywhere else gaff exits 0 or 1, because a gaff *failure* must never
block a session. A guard is not a failure. It is the operator saying
"not this call", and exit 2 is the only way the harness hears that.

So the rule refines rather than breaks. A guard that matches exits 2 on
purpose. Every other path still degrades, including a pattern that does
not compile. A broken guard blocks nothing, and `gaff check` names it,
because a guard that silently stops working is worse than no guard.

### Across hosts

A guard names a normalized tool and a field, not a host's payload
shape. The adapter maps a host's event onto `pre_tool_call`, so the
same guard works on any host with an adapter.

## Git hooks

gaff writes the hook scripts and dispatches them. Declare entries,
then run `gaff init --git`:

```yaml
git:
  - name: fmt
    on: [pre-commit]
    command: ["cargo", "fmt", "--check"]
  - name: test
    on: [git:pre-push]        # the domain prefix is optional
    command: ["cargo", "test"]
    required: false           # report the failure, run the rest anyway
```

| Field | Default | Description |
|-------|---------|-------------|
| `name` | required | The entry name, printed as the hook runs |
| `on` | required | The git hooks this entry runs on |
| `command` | required | The argv, run with the repo as the working directory |
| `required` | `true` | Stop the hook when this entry fails |

gaff installs these hooks: `pre-commit`, `prepare-commit-msg`,
`commit-msg`, `post-commit`, `pre-push`, `post-checkout`, `post-merge`,
and `pre-rebase`. git's own arguments are forwarded to the command, and
`pre-push` passes its ref list through on stdin.

### A git hook blocks, and an agent hook does not

These are two contracts, and gaff keeps them apart on purpose.

An agent hook must never block a session, so `gaff hook` exits 0 or 1
and never 2. A git hook exists *to* block: a non-zero exit aborts the
commit or the push. So `gaff githook` returns the failing command's
exit code. A broken config fails the hook, rather than skipping a check
that was meant to run.

### Installing, and what gaff will not touch

Running `gaff init --git` is the consent step, the same deliberate act
as `pre-commit install`. gaff writes one script per declared hook.

gaff never sets `core.hooksPath`. That setting names a single
directory, so it silently disables every hook another tool installed.
gaff writes individual files instead. When gaff finds a hook it did not
write, it keeps that file as `<hook>.local` and calls it first, so an
existing setup keeps working. `gaff init --git --uninstall` removes
gaff's scripts and restores what it kept.

Unlike a handler, a git command may be a bare name, and it may be
declared in the repo config. A repo's own lint and test commands are
the point of a git hook. The human who ran `gaff init --git` in that
repo is the consent.

## GitHub workflows

gaff cannot run a GitHub event, because it is not there when one fires.
So this domain is generated and checked, never executed.

```yaml
github:
  - name: ci
    on: [push, github:pull_request]
    branches: [main]
    steps:
      - use_git: fmt          # reuse the git entry's command
      - name: audit
        command: ["cargo", "audit"]
      - name: rust cache
        uses: Swatinem/rust-cache@v2      # a GitHub action
        with:
          shared-key: gate
```

`gaff init --github` renders each workflow to
`.github/workflows/<name>.yml`. `gaff check --github` compares the
render against the committed file, and exits 1 when one drifted, so CI
can run it.

| Field | Default | Description |
|-------|---------|-------------|
| `name` | required | The workflow name, and the filename |
| `on` | required | The triggering events |
| `branches` | none | Restrict a `push` or `pull_request` trigger |
| `runs_on` | `ubuntu-latest` | The runner |
| `steps` | required | Each step carries `command`, `use_git`, or `uses` (with optional `with` inputs, rendered in sorted key order) |

gaff renders `push`, `pull_request`, `merge_group`,
`workflow_dispatch`, `schedule`, and `release`.

### One check, written once

`use_git` names a `git:` entry and renders that entry's command into
the workflow. A check then runs in the git hook and in CI from a single
declaration. Change the command in one place, run `gaff init --github`,
and the workflow follows.

### The gates in CI: `gaff ci`

`gaff ci` runs the repo's declared git gates against HEAD, the way CI
must run them. It has three phases, and each one fails the run. The
config must load. No declared workflow may drift from its committed
file. Then the git entries for each requested hook run in declaration
order: `pre-commit`, then `pre-push`, or the set `--hook` names. A
requested hook with no entry fails the run: a CI checkout has no
installed hook script, so without that rule a branch that deletes an
entry runs nothing and lands green.

For `pre-push`, gaff synthesizes what git would send: `origin` and the
origin URL as arguments, and one ref line on stdin,
`refs/heads/<branch> <sha> refs/heads/<branch> <zero-sha>`. The sha is
HEAD's; the branch is `GITHUB_REF` when it names a branch, else the
checkout's branch, else `refs/heads/detached`. `GITHUB_SHA` never
changes what is tested; a mismatch prints a warning.

The gaff repository publishes a composite action, `cjohnhanson/gaff`,
that installs a pinned gaff and runs `gaff ci`. A repository that
declares its gates in `.gaff/gaff.yml` and one `github:` workflow with
that action as a step gets hooks and CI from the same declaration, so
the two cannot drift apart. The action installs no toolchain beyond
gaff; a Rust repository whose gates run clippy adds
`rustup component add clippy rustfmt` as a step of its own.

The render is deterministic, so regenerating an unchanged config
produces identical bytes and no diff.

## Host adapters and the event vocabulary

gaff names events for what they mean, not for what a host calls them.
An adapter maps its host's names onto this set:

| Normalized | Meaning | Flush point |
|------------|---------|-------------|
| `session_start` | A session begins or resumes | yes |
| `prompt` | The user submits a prompt | yes |
| `tool_call` | One tool call finished; gaff counts these | no |
| `tool_batch` | A batch of tool calls finished | yes |
| `stop` | The agent finished a turn | no |

A host event outside this set stays first-class. gaff forwards it and
permits nothing, rather than dropping it.

Write these names in a config, and read them in `gaff log`. A host's own
name, such as Claude Code's `PostToolBatch`, never appears above the
adapter.

Claude Code is the only implemented adapter. An adapter owns four
host-specific facts. It owns the payload mapping, and the map from its
event names onto the set above. It also owns its own event names for
registration, and the settings path that `gaff init` writes. `gaff hook` selects the adapter
from `GAFF_HOST`, or from the payload shape when that variable is
absent. `gaff init --host <name>` targets a named host.

gaff ships no guessed schema for an untested host. Adding one means
adding an `Adapter` constant with that host's real field names, taken
from its documentation.

## Exit codes

A gaff *failure* exits 0 or 1, never 2. Exit 2 is the code that blocks
an agent session, and no fault of gaff's may block one. A broken config
prints a warning on stderr, writes a `degraded` marker in the state
directory, and gaff continues without reminders. Run `gaff doctor` to see
the degradation.

Three things exit 2 on purpose, and none is a failure:

- A **guard** that refuses a tool call. That is the operator saying "not
  this call", and exit 2 is the only way the harness hears it.
- A **stop hook** that refuses a stop, because the work is not done.
- `gaff githook`, which relays the failing command's own exit code. A
  hook command that exits 2 makes gaff exit 2.
