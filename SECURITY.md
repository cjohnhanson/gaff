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

gaff runs from a coding agent's hooks. It refuses tool calls by a guard, injects text into a session, holds a stop, and runs commands a repository config declares.

Running a repository's declared command is the boundary worth attacking. gaff refuses to run handlers until a person trusts the repository, from their own shell, so cloning a repository never runs its code. A guard also decides whether an agent's tool call proceeds.

In scope:

- A document, a declaration, or a name reaching outside the directory
  it should be confined to.
- A fetch reaching a host or a path that no declaration named.
- Reading untrusted content leading to code execution.
- A repository's config causing a command to run before a person trusted it.
- An agent granting itself a right the trust boundary withholds.
- A guard that can be evaded by the shape of a command it should refuse.

Out of scope:

- A dependency advisory with no exploitable path through this tool.
  Report it to that dependency.
- Denial of service from a malformed local file, where the caller
  already controls that file.

## Known boundaries

Documented limits are not vulnerabilities. `src/confined.rs` carries a
`# What this does not cover` section in its module documentation. Read
it before reporting a traversal issue.
