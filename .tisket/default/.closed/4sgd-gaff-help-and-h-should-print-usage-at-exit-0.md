---
title: gaff --help and -h should print usage at exit 0
status: done
priority: '3'
assignee: null
due_date: null
labels:
- ux
depends_on: []
created: '2026-08-12T17:01:38Z'
updated: '2026-08-14T20:32:05Z'
---

Currently any unknown argument, including --help/-h, hits the unknown-command error path and exits 1. Conventional help flags should print usage and exit 0. Keep the never-exit-2 invariant.

## Scratch Notes

DONE: gaff --help/-h/help and --version/-V/version now print usage and the version at exit 0, hand-parsed to keep the never-exit-2 rule.
