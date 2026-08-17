//! The note reader, against a real repository.
//!
//! `reviewnote::note_body` reads the object database rather than
//! spawning git, because the single-binary rule bans a git process.
//! The first version of that read returned nothing for every commit,
//! and every unit test stayed green, because no test read a real note.
//! `gix` was missing its `sha1` feature, so opening the repository
//! failed and the check reported "no review note" for a commit that
//! carried one. Fail-closed, so no unreviewed change could land, and
//! no push could land either.
//!
//! Spawning git is allowed here. The rule covers `src/`, where gaff
//! decides things. A test may use git to build the repository it reads.

use gaff::reviewnote::{Missing, PushedRef, check};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run git in `dir` and return its stdout, trimmed. Panics on failure,
/// because a broken fixture must not read as a failing assertion.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap_or_else(|e| panic!("cannot run git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with one commit, in a directory that this test owns.
///
/// The name carries the process id and the test name, so two tests run
/// concurrently without sharing a path.
fn repo_with_one_commit(name: &str) -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("gaff-note-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test repository");
    git(&dir, &["init", "-q", "."]);
    std::fs::write(dir.join("f.txt"), "one").expect("write a file");
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "one"]);
    let sha = git(&dir, &["rev-parse", "HEAD"]);
    (dir, sha)
}

fn pushed(sha: &str) -> Vec<PushedRef> {
    vec![PushedRef {
        local_sha: sha.to_string(),
        remote_ref: "refs/heads/main".to_string(),
    }]
}

fn required() -> Vec<String> {
    vec!["fresh-eyes".to_string()]
}

#[test]
fn a_note_written_by_git_is_read_back() {
    let (dir, sha) = repo_with_one_commit("flat");
    let body = format!("signoff[fresh-eyes] PASS {sha} read the parser and every guard");
    git(&dir, &["notes", "--ref=reviews", "add", "-m", &body]);

    let verdicts = check(&dir, &pushed(&sha), &required());
    assert_eq!(verdicts.len(), 1);
    assert!(
        verdicts[0].faults.is_empty(),
        "a note git wrote was not read back: {:?}",
        verdicts[0].faults
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_commit_with_no_note_is_refused() {
    let (dir, sha) = repo_with_one_commit("bare");

    let verdicts = check(&dir, &pushed(&sha), &required());
    assert_eq!(verdicts[0].faults, vec![Missing::Note]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_note_under_a_fanout_directory_is_read_back() {
    // git splits the notes tree into `ab/cdef...` once it grows, and it
    // rewrites the layout as the tree changes. A note written flat
    // today sits under a fanout tomorrow, with no change to the commit
    // it annotates. Reaching that state through `git notes` needs a few
    // hundred commits, so the tree is built with plumbing instead.
    let (dir, sha) = repo_with_one_commit("fanout");
    let body = format!("signoff[fresh-eyes] PASS {sha} read the parser and every guard\n");

    let note_file = dir.join("note.txt");
    std::fs::write(&note_file, &body).expect("write the note body");
    let blob = git(&dir, &["hash-object", "-w", "note.txt"]);
    std::fs::remove_file(&note_file).expect("remove the note body");

    // `ab/` holds the rest of the sha, which is git's own two-character
    // split.
    let sub = git_mktree(&dir, &format!("100644 blob {blob}\t{}", &sha[2..]));
    let root = git_mktree(&dir, &format!("040000 tree {sub}\t{}", &sha[..2]));
    let commit = git(&dir, &["commit-tree", &root, "-m", "notes"]);
    git(&dir, &["update-ref", "refs/notes/reviews", &commit]);

    // The fixture is only worth trusting if git reads it as a note.
    let via_git = git(&dir, &["notes", "--ref=reviews", "show", &sha]);
    assert!(
        via_git.contains("signoff[fresh-eyes]"),
        "the fanout fixture is not a notes tree git recognises"
    );

    let verdicts = check(&dir, &pushed(&sha), &required());
    assert!(
        verdicts[0].faults.is_empty(),
        "a note under a fanout directory was not read: {:?}",
        verdicts[0].faults
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `git mktree` reads its entries on stdin, so it takes a writer rather
/// than an argument.
fn git_mktree(dir: &Path, entry: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .args(["mktree"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run git mktree");
    child
        .stdin
        .as_mut()
        .expect("mktree stdin")
        .write_all(format!("{entry}\n").as_bytes())
        .expect("write the tree entry");
    let out = child.wait_with_output().expect("wait for git mktree");
    assert!(out.status.success(), "git mktree failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_tag_pushed_at_a_commit_peels_to_it() {
    // git sends the tag's own sha on a pre-push line. The note sits on
    // the commit, so the check peels before it reads.
    let (dir, sha) = repo_with_one_commit("tag");
    let body = format!("signoff[fresh-eyes] PASS {sha} read the parser and every guard");
    git(&dir, &["notes", "--ref=reviews", "add", "-m", &body]);
    git(&dir, &["tag", "-a", "v1", "-m", "one"]);
    let tag_sha = git(&dir, &["rev-parse", "v1"]);
    assert_ne!(tag_sha, sha, "an annotated tag has its own object");

    let verdicts = check(&dir, &pushed(&tag_sha), &required());
    assert!(
        verdicts[0].faults.is_empty(),
        "a tag did not peel to the commit that carries the note: {:?}",
        verdicts[0].faults
    );
    let _ = std::fs::remove_dir_all(&dir);
}
