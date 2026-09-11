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

Only the user-scoped config may name a command, and gaff runs no handler
until a person trusts the repository from their own shell. Cloning a
repository therefore never runs its code. A guard decides whether an
agent's tool call proceeds.

In scope:

- A document, a declaration, or a name reaching outside the directory
  it should be confined to.
- A fetch reaching a host or a path that no declaration named.
- Reading untrusted content leading to code execution.
- A repository's config causing a command to run, or causing one to run
  before a person trusted the repository.
- An agent granting itself a right the trust boundary withholds.
- A guard that can be evaded by the shape of a command it should refuse.

Out of scope:

- A dependency advisory with no exploitable path through this tool.
  Report it to that dependency.
- Denial of service from a malformed local file, where the caller
  already controls that file.

## Known boundaries

Documented limits are not vulnerabilities. `gaff trust` and `gaff allow`
are kept from an agent by the built-in guard in `gaff hook`, which
refuses both from any Bash call an agent makes; neither command tests
for a terminal, and neither is a sandbox. An agent that can write your
home directory can still edit the files that record the grant.
`gaff docs configuration` states both limits.

`src/config.rs` documents what the section-path confinement covers, in
the comments on `read_section_body` and `read_confined`. Read those
before you report a traversal issue.
