---
title: 'prime: build it to the contract; then assemble primes as captured sections and make ecosystem.md policy-only'
status: done
priority: null
assignee: null
due_date: null
labels:
- prime
- sections
depends_on: []
created: 2026-08-16T19:08:20Z
updated: 2026-08-16T19:39:45Z
---

See mdstore's prime-contract issue. Two parts. (1) gaff prime, under 700 bytes: about string, what the tag the agent is about to see means without the literal [gaff: substring, that a refused call names its guard, config locations, five commands. No terminal-only claim for allow: that check was removed 2026-08-15 and the built-in guard is what refuses it. (2) In co.d: capture each tool's prime at build time into ~/.config/gaff/prime-<tool>.md, declare them as user sections with no refresh, raise the cap to 8192, and rewrite ecosystem.md as policy only with no command flags. Retire the hand-typed description.

## Scratch Notes

2026-08-16: all four primes shipped on main. almanac 9447394 (605 B), zettel 388cf88 (692 B), gaff 51390cc (591 B; parse_remind extracted so the test feeds remind lines back; USAGE and man page no longer say terminal-only for trust/allow; man exit-2 claim fixed), tisket 793d8ca (696 B; Repo::prime removed, prime runs outside a tracker, additional_instructions unread + reported by check, 44 fixtures updated, 28 missouri paths green). Each ships shape test + command-table walk. Next: co.d assembly (capture at build time as user sections, cap 8192, ecosystem.md policy-only).
2026-08-16: assembled and live. hms built prime-{tisket,zettel,gaff,almanac}.md as store paths; each byte-equal to the binary on PATH. Synthetic SessionStart flush delivered voice, four primes, ecosystem: 5726 B under 8192. CLAUDE.md Working Notes shrunk; Concurrency section rewritten to 'Concurrency and independent eyes' (co.d 2a0c7cd). Follow-up filed: ne8y (byte-total check).
