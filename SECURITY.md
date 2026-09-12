# Security policy

## Reporting

Do not open a public issue for a vulnerability.

Report it privately:
**https://github.com/cjohnhanson/gaff/security/advisories/new**

That opens a thread only you and the maintainer can read.

Include what an attacker gains, what they must already control to get
it, the affected commit, and steps that reproduce it.

## What happens next

gaff has one maintainer, so response is best effort. Expect a reply
within a week.

A confirmed report gets a fix and an advisory published together. You
are credited unless you ask otherwise.

## Scope

gaff runs from a coding agent's hooks. It refuses a tool call by a
guard, injects text into a session, holds a stop, and runs the commands
the user-scoped config declares.

On the hook path, only the user-scoped config may name a command, and
gaff runs no handler until a person trusts the working directory from
their own shell. Consent holds for that one directory. A repository's `git:` and `github:` entries name commands
too, and those run only after a person runs `gaff init --git` or
`gaff init --github` in that repository. Cloning a repository therefore
never runs its code. A guard decides whether an agent's tool call
proceeds.

In scope:

- A document, a declaration, or a name reaching outside the directory
  it should be confined to.
- A fetch reaching a host or a path that no declaration named.
- Reading untrusted content leading to code execution.
- A repository's config causing a command to run, or causing one to run
  before a person trusted the working directory.
- An agent granting itself a right the trust boundary withholds.
- A guard that can be evaded by the shape of a command it should refuse.

Out of scope:

- A dependency advisory with no exploitable path through this tool.
  Report it to that dependency.
- Denial of service from a malformed local file, where the caller
  already controls that file.

## Known boundaries

Documented limits are not vulnerabilities. The built-in guard in `gaff
hook` refuses `gaff trust` and `gaff allow`, which raises the cost of a
self-grant and makes one visible. It is not a boundary. The guard reads
a command as text, so a form it does not match reaches the shell
unrefused, and a quoted subcommand is one such form. Neither command
tests for a terminal. An agent that can write your home directory can
edit the files that record the grant directly. `gaff docs
configuration` states these limits.

`src/config.rs` documents what the section-path confinement covers, in
the comments on `read_section_body` and `read_confined`. Read those
before you report a traversal issue.
