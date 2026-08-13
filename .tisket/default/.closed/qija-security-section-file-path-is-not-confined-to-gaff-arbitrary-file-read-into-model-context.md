---
title: "SECURITY: section file path is not confined to .gaff/ — arbitrary file read into model context"
status: done
priority: 1
assignee:
labels: [security, bug]
depends_on: []
created: 2026-08-13T02:06:03Z
updated: "2026-08-13T17:01:45Z"
---

src/engine.rs:148 joins section.file onto .gaff/ and reads it with no normalization. A committed .gaff/gaff.yml with file: ../../../../etc/... or an absolute path reads any file the user can read straight into additionalContext at SessionStart. gaff check prints 'config ok'. Confirmed: a /tmp canary outside the repo came back inside the injected context. This is the RCE-on-clone / exfil class the original design review named; the section loader shipped without the guard. Fix: canonicalize the resolved path and reject anything outside the repo's .gaff/ (and referenced repo files); gaff check must fail on an escaping path.
