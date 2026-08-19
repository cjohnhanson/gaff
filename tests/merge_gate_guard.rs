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
    let dir = std::env::temp_dir().join(format!(
        "merge-gate-policy-{}-{}",
        std::process::id(),
        required.len() * 10 + vendored.len()
    ));
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
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(
            format!("refs/heads/topic {NOTELESS} refs/heads/main 0000000000000000000000000000000000000000\n")
                .as_bytes(),
        )
        .expect("the ref line writes");
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
