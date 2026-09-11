# 🪝 gaff

> A gaff is a pole with a sharp hook on the end. It lands what's
> drifting past.

gaff is a context-lifecycle handler for coding agents. It counts the hook
events of a session and re-injects context on a cadence. It also delivers
prime sections and advisory profiles.

## The problem

Context injected at session start decays as the conversation grows. It
moves into the low-attention middle of the context window. The decay
follows the number of messages and tool calls, not the wall clock.

The agent harness re-delivers no context on a cadence. Rules and skills
load on conditions. Reminders do not exist. A session of 300 tool calls
ends with its opening instructions effectively invisible.

## What gaff does

- **Counters.** gaff tallies the prompts and the tool calls of each
  session in an append-only ledger. A tool call counts once across its
  Pre, Post, and failure events.
- **Cadences.** A reminder re-injects its text every N tool calls or
  prompts. An agent can also schedule a one-shot reminder N tool calls
  into its own future, and gaff re-arms it after a context compaction.
- **Prime sections.** The session-start context, split into sections.
  Each section refreshes on its own cadence.
- **Handlers.** An external command whose output becomes context, on a
  cadence. Handlers live only in the user-scoped config, and a repo must
  be trusted with `gaff trust` before any command runs in it.
- **Guards.** A guard refuses a tool call that matches a regular
  expression. Declare one at user level and it applies in every repo.
  This is the feature that blocks, and it blocks on purpose.
- **Git hooks.** gaff writes the scripts in `.git/hooks/`, and they call
  back into gaff. One config declares the agent side and the git side. A
  hook gaff did not write is kept and called first.
- **GitHub workflows.** gaff generates them from the same config and
  checks them for drift. A check declared once runs in the git hook and
  in CI.
- **Profiles.** A profile is a named overlay that selects which entries
  are active and overrides their cadences. A transition policy states
  which profiles an agent may select for itself. Profiles are advisory,
  and gaff blocks nothing through them.

## Where config lives

`$HOME/.config/gaff/gaff.yml` holds what you want in every repo.
`.gaff/gaff.yml` holds what belongs to one repo, and it wins the names
it shadows. A repo never widens the profiles an agent may grant itself.
Handlers live only in `$HOME/.config/gaff/handlers.yml`.

## What gaff is not

- **Not an agent-hook dispatcher.** The harness's own hook system owns
  the matching, the timeouts, and the parallelism there. gaff registers
  as one handler. gaff does dispatch its own git hooks, because git has
  no dispatcher of its own.
- **Not an enforcement layer.** gaff refuses a tool call through a
  guard and a stop through a hold, and nothing else. It injects context
  only on the events whose output channel is the model's session
  framing. It never decorates a tool result.
- **Not a way to run repo-declared code from a hook.** On the agent
  path the repo-level config is data: sections, text, and cadences. A
  handler's command can only be declared in the user-scoped config. A
  repo's `git:` and `github:` entries do name commands, and they run
  only after a human runs `gaff init --git` or `gaff init --github` in
  that repo. Note the limit of that claim. A
  handler's command still *runs in* the repo's working directory, and
  tools like `git`, `make`, and `just` read executable settings from
  there. Handlers are therefore deny-by-default, and they need
  `gaff trust` per repo.

## Install

The package is `gaffr` on PyPI and npm, because `gaff` was taken. The
command is `gaff` everywhere, and both names install together.

Not released yet. Until the first tag, build from source:

```sh
cargo install --locked --git https://github.com/cjohnhanson/gaff
```

Requires Rust 1.88 and a C compiler. macOS and Linux, x86-64 and arm64.

From the first release onward:

```sh
cargo install --locked gaff
brew install cjohnhanson/tap/gaff
uv tool install gaffr
npm install -g gaffr
```

Or run it without installing:

```sh
uvx gaffr status
npx gaffr status
```

A release also carries prebuilt archives and a `.deb`, on the [releases
page](https://github.com/cjohnhanson/gaff/releases). Each archive holds
the binary and the man page. Install a `.deb` with `dpkg -i`: it is a
file, not a repository, so `apt-get install` does not reach it.

Check the install with `gaff --version`, and `gaff doctor` for what is
live in a clone.

## Using it

```
gaff init                          # register the hooks in the host's settings file
gaff remind "check CI" --after 10  # one-shot, N tool calls into the future
gaff status --session <id>         # counters, pending entries, one-shots
gaff check                         # validate .gaff/gaff.yml
gaff doctor                        # what is live in this clone
gaff init --git                    # write the git hook scripts
gaff init --github                 # generate the workflows
gaff check --github                # report a workflow that drifted
gaff trust                         # allow handlers to run in this repo
gaff check --handlers              # validate ~/.config/gaff/handlers.yml
gaff profile list                  # the declared profiles and who may set them
gaff profile set focus             # switch, and re-prime the sections
gaff log                           # what gaff injected into this session
gaff docs getting-started          # the bundled documentation
```

## Status

Every feature and every command listed above is built and runs. Nothing
is tagged yet, so the config keys and the output formats can still
change.

Two host adapters ship: Claude Code, and `generic`, which reads gaff's
own normalized field names for a host that speaks them. A host declares its
payload mapping, its event names, and its settings path in
`src/adapter.rs`, and nothing else in gaff changes. gaff ships no
guessed schema for a host nobody has tested.

A missouri state-graph suite of 28 paths and the cargo unit tests cover
this. The suite's error-surface path checks that a gaff failure exits 0
or 1, never the blocking code 2. Exit 2 belongs to a guard that refuses
a tool call, a stop hook that refuses a stop, `gaff run` reporting an
agent's refusal, and `gaff githook` relaying the failing command's own
code.

## Related

- [tisket](https://github.com/cjohnhanson/tisket) — issue tracker. Markdown issues with YAML frontmatter, in the repository
- [zettel](https://github.com/cjohnhanson/zettel) — zettelkasten notes for a repository
- [almanac](https://github.com/cjohnhanson/almanac) — agent skill index, over pluggable sources
- [missouri](https://github.com/cjohnhanson/missouri) — end-to-end tests as directed graphs of filesystem states
- [mdstore](https://github.com/cjohnhanson/mdstore) — the frontmattered markdown library the other three store documents with

## License

MIT.
