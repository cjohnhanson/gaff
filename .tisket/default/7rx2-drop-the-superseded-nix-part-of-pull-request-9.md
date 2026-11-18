---
title: Drop the superseded Nix part of pull request 9
status: todo
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-11-18T15:43:49Z
updated: 2026-11-18T15:43:49Z
---

Pull request #26 already fixed the Nix build and test PATH handling that pull request #9 targets. Pull request #9 also contains vendored review skills, workflows, and docs that #26 does not cover.

Done when:

- Pull request #9 is closed, or a rebase removes its changes to `flake.nix` and the PATH handling in `tests/merge_gate_guard.rs`.
