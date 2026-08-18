//! The note reader, against a real repository.
//!
//! `reviewnote::note_body` reads the object database rather than
//! spawning git, because the single-binary rule bans a git process.
//! The first version of that read returned nothing for every commit,
//! and every unit test stayed green, because no test read a real note.
//! `gix` was missing its `sha1` feature, so opening the repository
//! failed and the check reported "no review note" for a commit that
//! carried one. Fail-closed, so no unreviewed change could land, and
//! no push could complete either.
//!
//! Spawning git is allowed here. The rule covers `src/`, where gaff
//! decides things. A test may use git to build the repository it reads.

use gaff::reviewnote::{Missing, PushedRef, check};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run git in `dir` and return its stdout, trimmed. Panics on failure,
/// because a broken fixture must not read as a failing assertion.
fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        // The machine's own git config must not reach the fixture. A
        // contributor with `commit.gpgsign = true` turned every test
        // here red, because the fixture commit asked for a signature.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    // Git exports these to every hook it runs, and the merge gate runs
    // this suite from pre-push. An inherited GIT_DIR sends every
    // fixture at the real repository: `git init` reinitialises it,
    // `git commit` fires its pre-commit hooks, and the fixture commit
    // lands on the branch under test. Measured on 2026-08-18, when a
    // fixture commit named "one" became the tip of a branch.
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_command(dir)
        .args(args)
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
    let mut child = git_command(dir)
        .args(["mktree"])
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
fn the_fixture_command_drops_the_environment_git_gives_a_hook() {
    // The merge gate runs this suite from pre-push, where git exports
    // GIT_DIR and its siblings. Inherited, they aim every fixture at
    // the real repository. This asserts the removals rather than the
    // symptom, because the symptom is a commit on the branch under
    // test and a test must not produce one to prove a point.
    let cmd = git_command(Path::new("."));
    let removed: Vec<&str> = cmd
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .filter_map(|(key, _)| key.to_str())
        .collect();
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        assert!(
            removed.contains(&var),
            "{var} reaches the fixture, so a hook run writes to the real repository"
        );
    }
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

#[test]
fn a_notes_ref_that_holds_no_entry_for_this_commit_is_refused() {
    // Distinct from a repository with no notes ref at all. That case
    // stops at `find_reference`; this one walks the tree and finds
    // nothing, which is the path a real repository takes.
    let (dir, sha) = repo_with_one_commit("miss");
    let body = format!("signoff[fresh-eyes] PASS {sha} read the parser and every guard");
    git(&dir, &["notes", "--ref=reviews", "add", "-m", &body]);

    // A second commit, annotated by nothing.
    std::fs::write(dir.join("f.txt"), "two").expect("write a file");
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "two"]);
    let second = git(&dir, &["rev-parse", "HEAD"]);
    assert_ne!(second, sha);

    let verdicts = check(&dir, &pushed(&second), &required());
    assert_eq!(
        verdicts[0].faults,
        vec![Missing::Note],
        "a note belonging to another commit was read for this one"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reviews_check_takes_no_argument() {
    // The man page says the command takes none, and a test binds that
    // claim to the page. Nothing bound it to the code, so the flag
    // could return without failing anything.
    let out = Command::new(env!("CARGO_BIN_EXE_gaff"))
        .args(["reviews", "check", "--head", "abcdef0"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run gaff");
    assert!(!out.status.success(), "`reviews check --head` was accepted");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("unexpected argument"),
        "the refusal does not name the argument: {said}"
    );
}

/// Run `gaff reviews check` in `dir` with `stdin`, and return the exit
/// code and stderr.
///
/// These cases drive the binary rather than a function. A reviewer
/// mutated both stdin guards at their call sites, left the functions
/// correct, and the whole suite stayed green while the original bugs
/// came back. A unit test on a function does not bind the line that
/// calls it.
fn reviews_check(dir: &Path, stdin: &str) -> (Option<i32>, String) {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_gaff"))
        .args(["reviews", "check"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run gaff reviews check");
    child
        .stdin
        .as_mut()
        .expect("gaff stdin")
        .write_all(stdin.as_bytes())
        .expect("write the ref lines");
    let out = child.wait_with_output().expect("wait for gaff");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// A repository the check will read: one commit and a review policy.
fn repo_with_a_policy(name: &str) -> (PathBuf, String) {
    let (dir, sha) = repo_with_one_commit(name);
    std::fs::create_dir_all(dir.join(".gaff")).expect("create .gaff");
    std::fs::write(dir.join(".gaff/gaff.yml"), "reviews:\n  - fresh-eyes\n")
        .expect("write the policy");
    (dir, sha)
}

#[test]
fn a_sha_that_is_not_ascii_refuses_rather_than_panicking() {
    // Exit 101 is a panic. The gate must refuse, not crash.
    let (dir, _sha) = repo_with_a_policy("nonascii");
    let line =
        "refs/heads/main abcdef\u{e9} refs/heads/main 0000000000000000000000000000000000000000\n";
    let (code, said) = reviews_check(&dir, line);
    assert_eq!(code, Some(1), "gaff panicked or passed: {said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_truncated_ref_line_refuses_the_push() {
    // git writes four fields. Fewer means the stream was cut, and
    // reading that as "nothing to check" let a caller who lost the
    // stream past the gate.
    let (dir, _sha) = repo_with_a_policy("cutstream");
    let (code, said) = reviews_check(&dir, "refs/heads/main\n");
    assert_eq!(code, Some(1), "a truncated stream passed: {said}");
    assert!(
        said.contains("truncated"),
        "the refusal does not name the truncation: {said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_up_to_date_push_sends_no_ref_and_is_allowed() {
    // git runs a pre-push hook with no lines when the remote already
    // holds everything. Refusing that blocked every no-op push.
    let (dir, _sha) = repo_with_a_policy("uptodate");
    let (code, said) = reviews_check(&dir, "");
    assert_eq!(code, Some(0), "an up-to-date push was refused: {said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ci_refuses_a_head_that_does_not_name_a_branch() {
    // The gate exempts a notes ref, because pushing review records
    // proposes no change. `gaff ci` builds its ref line from HEAD, so
    // a HEAD pointing at `refs/notes/reviews` synthesized an exempt
    // line and the run certified nothing while reporting success. One
    // `git symbolic-ref` was the whole attack.
    let (dir, sha) = repo_with_a_policy("headnotes");
    // A declared pre-push entry, so the run reaches HEAD resolution
    // rather than stopping at "no git entry runs on pre-push".
    std::fs::write(
        dir.join(".gaff/gaff.yml"),
        "reviews:\n  - fresh-eyes\ngit:\n  - name: gate\n    on: [pre-push]\n    command: [true]\n",
    )
    .expect("write the policy and a hook entry");
    git(&dir, &["update-ref", "refs/notes/reviews", &sha]);
    git(&dir, &["symbolic-ref", "HEAD", "refs/notes/reviews"]);

    let out = Command::new(env!("CARGO_BIN_EXE_gaff"))
        .args(["ci"])
        .current_dir(&dir)
        .output()
        .expect("run gaff ci");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "gaff ci certified a run whose HEAD names a notes ref: {said}"
    );
    assert!(
        said.contains("not a branch"),
        "the refusal does not name the cause: {said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
