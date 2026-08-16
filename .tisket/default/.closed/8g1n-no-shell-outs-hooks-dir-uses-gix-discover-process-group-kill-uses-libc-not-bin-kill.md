---
title: 'no shell-outs: hooks_dir uses gix discover; process-group kill uses libc, not /bin/kill'
status: done
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-08-16T19:54:34Z
updated: 2026-08-16T21:26:05Z
---

## Goal

`githook::hooks_dir` asks `git rev-parse --git-common-dir` for the hooks directory; it uses `gix::discover` and `common_dir()` instead. `handler` kills a timed-out handler's process group with `/bin/kill`; it uses `libc::killpg`. No `Command::new("git")` or `/bin/kill` remains. Handler and githook commands themselves are user-declared and stay spawned: that is the product.

## Why

Single-binary rule; see mdstore's issue of the same title. Introduced 2026-08-14 in the worktree-hooks fix.

## Scratch Notes

2026-08-16: fixed locally (dfbda1a); push after the QA review workflow.
2026-08-16: done. hooks_dir on gix-discover + commondir (worktree test); kill_group on nix killpg. QA: two pre-existing minors filed (init --git from subdir; stdout lost on deadline). Pushed 936f25f.
