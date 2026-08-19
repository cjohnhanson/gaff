#!/bin/sh
# The merge gate runs on pre-push. Direct push is how these repos
# merge, so pre-push is the merge check.
#
# A push needs passing tests and a review note that names every review
# .gaff/gaff.yml declares. `gaff reviews check` holds the note
# requirement, so no review name appears here. The missouri suite runs
# where tests/missouri exists.
#
# The suites test the working tree, not the pushed commit. A pre-push
# hook is also advisory, because `--no-verify` skips it, so the
# required CI check is what enforces this.
set -e

# A change is judged by the policy already in force, not by the policy
# it ships. Without this, a branch that empties `reviews:` passes its
# own check with no sign-off, because the gate reads the file from the
# branch it is judging. Comparing against the merge base closes that.
# A branch that adds a review is fine; one that drops a review is not,
# and the drop lands only once a change carrying it has been reviewed
# under the older, stricter policy.
base_policy=$(git show origin/main:.gaff/gaff.yml 2>/dev/null || true)
if [ -n "$base_policy" ]; then
  base_names=$(printf '%s\n' "$base_policy" |
    awk '/^reviews:/ {inside=1; next} inside && /^  - / {print $2; next} inside && !/^  - / {exit}')
  for name in $base_names; do
    # Only a review the merge base could actually perform is protected.
    # A name with no criteria on the base was never enforceable, so
    # replacing it is not a weakening.
    git cat-file -e "origin/main:.agents/skills/$name/SKILL.md" 2>/dev/null || continue
    if ! gaff reviews | grep -qx "$name"; then
      echo "merge-gate: $name is required on origin/main and this branch drops it." >&2
      echo "  A branch cannot weaken the policy that judges it. Restore the name," >&2
      echo "  or land the removal through a change reviewed under the current policy." >&2
      exit 1
    fi
  done
fi

# Every required review needs vendored criteria, and every vendored
# review needs to be required. A name with no criteria is a review
# nobody can perform. A criterion nobody requires is a check that one
# edit dropped. Checking both directions is what stops that edit.
required=$(gaff reviews)
for name in $required; do
  if [ ! -f ".agents/skills/$name/SKILL.md" ]; then
    echo "merge-gate: $name is required and has no criteria in .agents/skills." >&2
    echo "  Vendor it: almanac add github:cjohnhanson/skills --path skills/$name --name $name --accept" >&2
    exit 1
  fi
done
for dir in .agents/skills/review-*/; do
  [ -d "$dir" ] || continue
  name=${dir#.agents/skills/}
  name=${name%/}
  if ! printf '%s\n' "$required" | grep -qx "$name"; then
    echo "merge-gate: $name is vendored and required by nothing." >&2
    echo "  Name it under reviews: in .gaff/gaff.yml, or remove it." >&2
    exit 1
  fi
done


# git sends the ref list on stdin. The first reader spends the stream.
# Capture it before any other program can read it. If a test runner
# read stdin first, the check below would see EOF and check nothing.
gate_refs=$(cat)

# The escape needs both the marker and CARGO, which only a cargo-run
# process sets, so a plain shell cannot turn the tests off with one
# variable. A pushing developer never has CARGO set. The policy check
# above runs either way.
if [ -z "${MERGE_GATE_SKIP_TESTS:-}" ] || [ -z "${CARGO:-}" ]; then
echo "merge-gate: cargo test"
# --all-features, because a feature that is off by default is still
# shipped code. The gate once built without mcp and never compiled it.
# Capture the output. On red, the failing test's name is the first
# thing a reader needs, and /dev/null once hid it from the CI log.
test_out=$(cargo test --workspace --all-features --quiet 2>&1 </dev/null) || {
	echo "merge-gate: cargo test failed. Nothing merges on red tests." >&2
	printf '%s\n' "$test_out" | tail -40 >&2
	exit 1
}

# The CI runner has no nix, but it preinstalls the packages the
# suites declare. When CI is set, missouri uses the preinstalled
# backend. A local run keeps the nix backend.
if [ -n "${CI:-}" ]; then
	MISSOURI_SANDBOX=preinstalled
	export MISSOURI_SANDBOX
fi

fi

if [ -d tests/missouri ] && { [ -z "${MERGE_GATE_SKIP_TESTS:-}" ] || [ -z "${CARGO:-}" ]; }; then
	command -v missouri >/dev/null || {
		echo "merge-gate: missouri is not on PATH and tests/missouri exists." >&2
		exit 1
	}
	echo "merge-gate: missouri run"
	out=$(cd tests/missouri && missouri run </dev/null 2>&1) || {
		echo "merge-gate: the missouri suite failed. Nothing merges on a red suite." >&2
		printf '%s\n' "$out" | tail -20 >&2
		exit 1
	}
	# The exit code decides. The summary check adds a second gate: the
	# run must show one or more passed paths and zero failures. An empty
	# suite does not pass.
	printf '%s\n' "$out" | grep -E '[1-9][0-9]* passed, 0 failed' >&2 || {
		echo "merge-gate: the suite reported no passing path. An empty suite gates nothing." >&2
		exit 1
	}
fi

command -v gaff >/dev/null || {
	echo "merge-gate: gaff is not on PATH, so the review check cannot run." >&2
	echo "  cargo install --git https://github.com/cjohnhanson/gaff" >&2
	exit 1
}

# Last in the pipeline, always. A POSIX pipeline exits with its final
# command, so anything after this would discard the refusal.
printf '%s\n' "$gate_refs" | gaff reviews check
echo "merge-gate: ok"
