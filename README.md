# gaff

A configurable hook handler for coding agent sessions.

gaff is one of the [eng.rip](https://eng.rip/) tools.

## Highlights

- [Refuses tool calls](https://gaff.eng.rip/guides/enforce-tool-rules/) before they run.
- [Project instructions](https://gaff.eng.rip/guides/refresh-project-context/) delivered at session start and restored after compaction.
- [Git hooks](https://gaff.eng.rip/guides/integration/git/) with checks that block commits or pushes when they fail.
- [Runs the same checks in CI](https://gaff.eng.rip/guides/run-gates-in-ci/) from a single declaration shared with Git hooks.
- [Requires review sign-offs](https://gaff.eng.rip/guides/require-reviews/) before pushes and on pull requests, for every review named by the repository.
- [Host adapters](https://gaff.eng.rip/guides/integration/claude-code/) for Claude Code, the Codex CLI, the Antigravity CLI, and [generic hook hosts](https://gaff.eng.rip/guides/integration/generic-hosts/).

## Installation

Supports macOS and Linux.

The package is `gaff` on Cargo and `gaffr` on PyPI and npm, and every method installs the `gaff` command.

Install with one of:

```sh
brew install cjohnhanson/tap/gaff
cargo install --locked gaff
uv tool install gaffr
npm install -g gaffr
```

Prebuilt binaries are on the [GitHub releases page](https://github.com/cjohnhanson/gaff/releases); see the [installation guide](https://gaff.eng.rip/getting-started/installation/) for every method.

## Example

Declare project instructions, Git hooks, and required review skills in the repository's `.gaff/gaff.yml`:

```yaml
sections:
  - name: conventions
    file: sections/conventions.md

git:
  - name: test
    on: [pre-commit]
    command: ["cargo", "test"]

reviews:
  - review-code
```

The section `.gaff/sections/conventions.md` contains the project instructions:

```markdown
Stage files by name. Run the tests before every commit.
```

Register the hooks, check the configuration, and list the required reviews with `gaff reviews`:

```console
$ gaff init
gaff hooks registered in .claude/settings.local.json

$ gaff init --git
installed: pre-commit

$ gaff check
config ok: 0 reminder(s), 1 section(s), 0 guard(s), cap 4096 bytes

$ gaff reviews
review-code

```

## Documentation

Read the guides at [gaff.eng.rip](https://gaff.eng.rip/). Run `gaff docs` to list the bundled topics, or `gaff docs <topic>` to print one.

For guides that combine tools, see [eng.rip guides](https://eng.rip/#guides).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE).
