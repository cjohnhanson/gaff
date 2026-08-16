---
title: init --git from a subdirectory finds no repo config
status: todo
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-08-16T21:02:18Z
updated: 2026-08-16T21:02:18Z
---

## Problem

`gaff init --git` run from `<repo>/sub` says "the config declares no git entries" because config loading joins `.gaff/gaff.yml` onto cwd. `hooks_dir` walks up to the repo; config loading does not. Found by QA on 2026-08-16 (`src/config.rs:873`, `src/cli.rs:1429`).

## Fix

Resolve the repo root (the same discovery `hooks_dir` uses) and load `.gaff/gaff.yml` from there for `init --git`, `init --github`, and `githook`.
