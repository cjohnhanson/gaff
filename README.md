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
  cadence. Handlers live only in the user-scoped config, and a directory
  must be trusted with `gaff trust` before any command runs in it.
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
`.gaff/gaff.yml` holds what belongs to one repo. A repo entry under a
name the user already declared is refused with a warning, and the
user's entry stands. A repo never widens the profiles an agent may
grant itself.
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
  handler's command still inherits the working directory gaff was
  called in, and tools like `git`, `make`, and `just` read executable
  settings from there. Handlers are therefore deny-by-default, and they need
  `gaff trust` in each directory the agent starts in.

## Install

The command is `gaff`, whichever package you install.

**[Archives of precompiled binaries are available for macOS and
Linux.](https://github.com/cjohnhanson/gaff/releases)** The Linux
binaries are static executables. Each archive holds the binary, its man
page, the README and the licence. There is no Windows build.

| Package manager | Package | Command |
| --- | --- | --- |
| [Homebrew](https://brew.sh) | [cjohnhanson/tap/gaff](https://github.com/cjohnhanson/homebrew-tap) | `brew install cjohnhanson/tap/gaff` |
| [Cargo](https://doc.rust-lang.org/cargo/) | [gaff](https://crates.io/crates/gaff) | `cargo install --locked gaff` |
| [uv](https://docs.astral.sh/uv/) | [gaffr](https://pypi.org/project/gaffr/) | `uv tool install gaffr` |
| [npm](https://www.npmjs.com) | [gaffr](https://www.npmjs.com/package/gaffr) | `npm install -g gaffr` |
| [Nix](https://nixos.org) | [github:cjohnhanson/gaff](https://github.com/cjohnhanson/gaff) | `nix profile install github:cjohnhanson/gaff` |

On Debian or Ubuntu, download the `.deb` from the [releases
page](https://github.com/cjohnhanson/gaff/releases) and install it:

```sh
wget https://github.com/cjohnhanson/gaff/releases/download/v0.1.4/gaff_0.1.4-1_amd64.deb
sudo dpkg -i gaff_0.1.4-1_amd64.deb
```

To run it once without installing anything:

```sh
uvx gaffr doctor
npx gaffr doctor
nix run github:cjohnhanson/gaff -- doctor
```

### Building

gaff is written in Rust, so you need a [Rust
installation](https://www.rust-lang.org/) to compile it. gaff compiles
with Rust 1.88 or newer. A C compiler is needed as well, which
`aws-lc-sys` uses for its cryptography.

To build gaff:

```sh
git clone https://github.com/cjohnhanson/gaff
cd gaff
cargo build --release
./target/release/gaff --version
```

## Using it

A repo with this `.gaff/gaff.yml` keeps a section and a reminder alive,
runs `cargo fmt --check` before each commit and on each pull request,
and offers a `focus` profile that drops the reminder:

```yaml
sections:
  - name: conventions
    file: sections/conventions.md
    refresh: {tool_calls: 25}

reminders:
  - name: scratch
    every: {tool_calls: 20}
    text: "Update your working notes before they go stale."

profiles:
  focus:
    disable: [scratch]
transitions:
  agent_may_set: [focus]

git:
  - name: fmt
    on: [pre-commit]
    command: ["cargo", "fmt", "--check"]

github:
  - name: ci
    on: [pull_request]
    steps:
      - use_git: fmt
```

The section's text lives in `.gaff/sections/conventions.md`:

```markdown
Stage files by name. Run the tests before every commit.
```

Register the agent hooks, write the git hook and the workflow, and
check the result:

```console
$ gaff init
gaff hooks registered in .claude/settings.local.json

$ gaff init --git
installed: pre-commit

$ gaff init --github
generated: .github/workflows/ci.yml

$ gaff check
config ok: 1 reminder(s), 1 section(s), 0 guard(s), cap 4096 bytes

$ gaff check --github
ok      ci

$ gaff doctor
config:  ok (1 reminder(s), 1 section(s))
state:   [..]/.local/state/gaff/[..]
hooks:   registered (local (7)), all 7 events
guards:  none active
         (a broken or unreadable user config disarms every guard)
handlers: none declared

```

Inside a session, the host sets the session id. From a terminal, pass
it with `--session`:

```console
$ gaff remind "check CI" --after 10 --session demo

$ gaff status --session demo
{"oneshots":[{"at":10,"fired":false,"id":"r1"}],"pending":[],"prompts":0,"tool_calls":0}

$ gaff profile list
focus	agent or human

$ gaff profile set focus --session demo
profile set to `focus`; the next flush re-primes the sections

$ gaff log --session demo
no injections recorded for session demo

```

Handlers run only in a directory you trust:

```console
$ gaff trust
this directory may now run handlers: [CWD]
consent is per directory. Another directory needs its own `gaff trust`.

$ gaff check --handlers
no handlers declared

```

`gaff docs` lists the bundled documentation, and `gaff docs <topic>`
prints a topic:

```console
$ gaff docs
Topics (gaff docs <topic>):
  getting-started      Install gaff, wire the hooks, and watch a section fire
  cli-reference        The help gaff prints, for each command that has its own
  configuration        Every key in .gaff/gaff.yml: sections, reminders, guards, and cadences
  how-it-works         Why injected context decays, and how counting, arming, and flushing answer it

```

## Status

Every feature and every command listed above is built and runs. The
version is below 1.0, so the config keys and the output formats can
still change.

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
- [kersh](https://github.com/cjohnhanson/kersh) — the declarative agent runner a hook agent calls by default

## License

MIT.
