---
title: CI builds the flake package on every pull request
status: done
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-11-18T15:43:49Z
updated: 2026-11-18T15:48:21Z
---

No CI job builds the flake package, so a pull request can pass every check and still break `nix build .#default`. `nix flake check` evaluates the package but does not build it.

Done when:

- `.gaff/gaff.yml` declares a `nix` job, and `gaff init --github` generates `.github/workflows/nix.yml`.
- The job runs on every pull request, installs Nix with `cachix/install-nix-action@v31`, and runs `nix build .#default -L --no-link`.
- The job fails when the package's check phase fails.
- The change leaves branch protection unchanged. Requiring `nix` remains a separate repository decision.
