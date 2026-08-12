---
title: "gaff --help and -h should print usage at exit 0"
status: todo
priority: 3
assignee:
labels: [ux]
depends_on: []
created: "2026-08-12T17:01:38Z"
updated: "2026-08-12T17:01:38Z"
---

Currently any unknown argument, including --help/-h, hits the unknown-command error path and exits 1. Conventional help flags should print usage and exit 0. Keep the never-exit-2 invariant.
