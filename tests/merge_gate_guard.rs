//! The merge gate's policy reconciliation.
//!
//! `scripts/merge-gate.sh` refuses a required review that has no
//! vendored criteria, and a vendored review that nothing requires.
//! Checking both directions is what stops one edit from dropping a
//! check quietly.
//!
//! These tests build a fixture directory and run the script there, so
//! the policy and the criteria can disagree without touching this
//! repository. `gaff reviews` reads `.gaff/gaff.yml` from the working
//! directory, so a directory is the whole seam.

use std::io::Write as _;
use std::process::{Command, Stdio};

/// A sha with no review note. The empty tree object always exists and
/// never carries one.
const NOTELESS: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Runs the gate in a fixture directory that this test builds, so the
/// policy and the vendored criteria can disagree without touching the
/// repository. `gaff reviews` reads `.gaff/gaff.yml` from the working
/// directory, and the reconciliation reads `.agents/skills` from it, so
/// a directory is the whole seam. The script needs none of its own.
fn run_gate_in(required: &[&str], vendored: &[&str]) -> (i32, String) {
    let root = env!("CARGO_MANIFEST_DIR");
    // The names are the key. Two fixtures with one count each ran in one
    // directory at the same time and read each other's files.
    let dir = std::env::temp_dir().join(format!(
        "merge-gate-policy-{}-{}-{}",
        std::process::id(),
        required.join("+"),
        vendored.join("+")
    ));
    assert!(
        dir.starts_with(std::env::temp_dir()),
        "the fixture must live under the temp directory, not at {}",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".gaff")).expect("the fixture directory is made");
    std::fs::create_dir_all(dir.join("scripts")).expect("the scripts directory is made");
    std::fs::copy(
        format!("{root}/scripts/merge-gate.sh"),
        dir.join("scripts/merge-gate.sh"),
    )
    .expect("the gate is copied");

    let mut yml = String::from("reviews:\n");
    for name in required {
        yml.push_str("  - ");
        yml.push_str(name);
        yml.push('\n');
    }
    std::fs::write(dir.join(".gaff/gaff.yml"), yml).expect("the policy is written");
    for name in vendored {
        let d = dir.join(".agents/skills").join(name);
        std::fs::create_dir_all(&d).expect("a criteria directory is made");
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: fixture\n---\n\nbody\n"),
        )
        .expect("a criteria file is written");
    }

    let mut child = Command::new("sh")
        .arg("scripts/merge-gate.sh")
        .current_dir(&dir)
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITHUB_EVENT_NAME")
        .env_remove("GITHUB_EVENT_PATH")
        .env("MERGE_GATE_SKIP_TESTS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("merge-gate.sh runs");
    // Every refusal this helper drives comes from the configuration
    // checks, and those run before the gate reads stdin. The gate can
    // therefore exit, and close the pipe, before this write lands. A
    // broken pipe is the refusal under test, so only that kind passes.
    if let Err(e) = child.stdin.as_mut().expect("stdin is piped").write_all(
        format!(
            "refs/heads/topic {NOTELESS} refs/heads/main 0000000000000000000000000000000000000000\n"
        )
        .as_bytes(),
    ) {
        assert_eq!(
            e.kind(),
            std::io::ErrorKind::BrokenPipe,
            "the ref line writes: {e}"
        );
    }
    let out = child.wait_with_output().expect("the gate finishes");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn a_required_review_with_no_criteria_refuses() {
    // A name nobody can review. One typo in the policy produces it, and
    // the gate would otherwise wait for a sign-off no reviewer can give.
    let (code, out) = run_gate_in(&["review-tests", "review-tets"], &["review-tests"]);
    assert_ne!(code, 0, "a required name with no criteria passed: {out}");
    assert!(
        out.contains("review-tets is required and has no criteria"),
        "expected the missing-criteria refusal, got: {out}"
    );
    // The note check refuses this fixture too, so a nonzero exit proves
    // nothing on its own. The gate has to stop here, before it runs.
    assert!(
        !out.contains("no review note on"),
        "the gate printed the refusal and carried on to the note check: {out}"
    );
}

#[test]
fn a_vendored_review_nobody_requires_refuses() {
    // Dropping a name from the policy leaves its criteria in the tree.
    // Checking this direction is what stops one edit from dropping a
    // check quietly.
    let (code, out) = run_gate_in(&["review-tests"], &["review-tests", "review-docs"]);
    assert_ne!(code, 0, "a vendored review nobody requires passed: {out}");
    assert!(
        out.contains("review-docs is vendored and required by nothing"),
        "expected the orphan refusal, got: {out}"
    );
    assert!(
        !out.contains("no review note on"),
        "the gate printed the refusal and carried on to the note check: {out}"
    );
}

#[test]
fn a_review_name_is_a_fixed_string() {
    // The orphan check greps the required list for each vendored name.
    // A vendored `review-.ests` read as a regular expression matches the
    // required `review-tests`, so the orphan once passed as required. A
    // name matches as a fixed string.
    let (code, out) = run_gate_in(&["review-tests"], &["review-tests", "review-.ests"]);
    assert_ne!(code, 0, "a name that matched by pattern passed: {out}");
    assert!(
        out.contains("review-.ests is vendored and required by nothing"),
        "expected the orphan refusal, got: {out}"
    );
}

#[test]
fn matched_lists_reach_the_note_check() {
    // Neither guard fires when the two lists agree, so the refusal that
    // follows is the note check rather than the reconciliation.
    let (code, out) = run_gate_in(&["review-tests"], &["review-tests"]);
    assert_ne!(code, 0, "a note-less sha passed: {out}");
    assert!(
        out.contains("no review note on"),
        "expected the note refusal, got: {out}"
    );
}
