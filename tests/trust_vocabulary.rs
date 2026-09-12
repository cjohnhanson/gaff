//! One term for consent, across every surface that reports it.
//!
//! Consent is per directory: `handler::trust` records one canonical
//! path and `handler::is_trusted` compares against that path, with no
//! prefix match. The messages once said "repo", so `gaff trust` in a
//! subdirectory reported success while `gaff doctor` one level up
//! reported the repository was not trusted. One state, two answers.
//!
//! Nothing asserted on those strings, so a rename that reached some of
//! them and not the rest passed a green suite. These tests drive the
//! built binary and read what a user actually sees.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch HOME holding one declared handler, and a repository
/// directory with a subdirectory under it.
struct Probe {
    root: PathBuf,
}

impl Probe {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("gaff_trust_vocab_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.config/gaff")).expect("the scratch home is made");
        std::fs::create_dir_all(root.join("repo/sub")).expect("the repo directory is made");
        // `doctor` reports trust only where a handler is declared, so
        // the probe declares one. The command never runs.
        std::fs::write(
            root.join("home/.config/gaff/handlers.yml"),
            "handlers:\n  - name: probe\n    events: [stop]\n    command: [\"/bin/echo\", \"hi\"]\n",
        )
        .expect("the handler config writes");
        // An agent as well, in the user config where agents are
        // declared. `gaff run` reaches the untrusted refusal only for a
        // declared agent, and that refusal is one of the three messages
        // the rename rewrote with no test driving it.
        std::fs::write(
            root.join("home/.config/gaff/gaff.yml"),
            "agents:\n  - name: probe-agent\n    context: [\"/bin/echo\", \"ctx\"]\n",
        )
        .expect("the user config writes");
        Self { root }
    }

    /// Runs the built binary with the scratch HOME, in `cwd` under the
    /// probe root. Returns stdout and stderr together.
    fn run(&self, cwd: &str, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_gaff"))
            .args(args)
            .current_dir(self.root.join(cwd))
            .env("HOME", self.root.join("home"))
            .env_remove("GAFF_STATE_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("gaff runs");
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
    }

    fn path(&self, cwd: &str) -> PathBuf {
        self.root
            .join(cwd)
            .canonicalize()
            .expect("the probe directory resolves")
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn trust_names_the_directory_it_recorded() {
    let p = Probe::new("names");
    let out = p.run("repo/sub", &["trust"]);
    assert!(
        out.contains("this directory may now run handlers"),
        "trust did not name the directory: {out}"
    );
    assert!(
        out.contains(p.path("repo/sub").to_string_lossy().as_ref()),
        "trust did not print the path it recorded: {out}"
    );
    // The new-grant branch is the one every user reaches first, and its
    // per-directory sentence was asserted only on the already-trusted
    // branch. Deleting it here left the suite green.
    assert!(
        out.contains("consent is per directory"),
        "the grant did not state the rule it is named for: {out}"
    );
}

#[test]
fn doctor_reports_the_directory_it_stands_in() {
    let p = Probe::new("doctor");
    // Grant consent in the subdirectory, then ask one level up. The
    // record does not reach the parent, and doctor must say so about
    // the directory rather than about the repository.
    let granted = p.run("repo/sub", &["trust"]);
    assert!(granted.contains("may now run handlers"), "{granted}");

    let here = p.run("repo/sub", &["doctor"]);
    assert!(
        here.contains("this directory is trusted"),
        "doctor disagreed with trust in the same directory: {here}"
    );

    let above = p.run("repo", &["doctor"]);
    assert!(
        above.contains("this directory is NOT trusted"),
        "doctor did not report the parent as untrusted: {above}"
    );
}

#[test]
fn a_subdirectory_of_a_trusted_directory_is_not_trusted() {
    // The direction the rule is actually about, and the one nothing
    // asserted. `is_trusted` compares one canonical path for equality,
    // with no prefix match, and every page states that a subdirectory
    // of a trusted directory is not itself trusted. Changing the
    // comparison to `starts_with` passed the whole suite.
    let p = Probe::new("subdir");
    let granted = p.run("repo", &["trust"]);
    assert!(granted.contains("may now run handlers"), "{granted}");

    let here = p.run("repo", &["doctor"]);
    assert!(
        here.contains("this directory is trusted"),
        "doctor disagreed with trust in the same directory: {here}"
    );

    let below = p.run("repo/sub", &["doctor"]);
    assert!(
        below.contains("this directory is NOT trusted"),
        "the grant reached a subdirectory, so consent is not per directory: {below}"
    );
}

/// Every phrasing that means repository-scoped consent.
///
/// One term for one concept. Two literal strings were not enough: the
/// rename reached the strings a guard happened to name and left the
/// rest, which is the shape this file exists to stop.
/// Claims that the built-in guard stops a self-grant outright.
///
/// It does not. The guard matches the command as text, and `gaff
/// "trust"` reaches the shell unrefused. Five prose surfaces carried
/// the absolute form, and correcting one left four, so the phrasings
/// are pinned rather than trusted to a sweep.
const ABSOLUTE_GUARD_CLAIM: [&str; 4] = [
    "cannot grant consent",
    "cannot grant this",
    "from any Bash call",
    "kept from an agent",
];

const REPO_SCOPED: [&str; 9] = [
    "this repo is",
    "in this repo",
    "untrusted repo",
    "trusted repo",
    "per repo",
    "per-repo",
    // The rename also left two statements of the old model that none
    // of the six above matched, one in the README and one in this
    // crate's own module header. A guard shaped to the phrasings that
    // happened to exist does not catch the next one.
    "the repo's working directory",
    "with the repo as its working directory",
    "repo may be hostile",
];

#[test]
fn no_surface_that_reports_consent_calls_it_a_repo() {
    // This is the assertion the incomplete rename needed. Each of these
    // reports consent, and each once said "repo" while its neighbor
    // said "directory". The untrusted path carries three of the
    // rewritten messages and was the half left unasserted.
    let p = Probe::new("vocab");

    // Untrusted first. Granting trust before these would route them
    // past the three refusals, which is the half that went unasserted.
    let untrusted: &[(&str, &[&str])] = &[
        ("repo", &["doctor"]),
        ("repo", &["--help"]),
        // `trust --help` is the surface the code owns, and it carried
        // the false security claim. It was the one page the guard did
        // not read. It grants nothing, so it is safe before the grant.
        ("repo", &["trust", "--help"]),
        ("repo", &["check", "--handlers"]),
        ("repo", &["run", "probe-agent"]),
    ];
    for (cwd, args) in untrusted {
        check_vocabulary(&p, cwd, args);
    }

    // Then grant, and read the trusted surfaces and the parent that the
    // grant does not reach.
    check_vocabulary(&p, "repo", &["trust"]);
    for (cwd, args) in [
        ("repo", ["doctor"].as_slice()),
        ("repo/sub", ["doctor"].as_slice()),
        ("repo/sub", ["check", "--handlers"].as_slice()),
    ] {
        check_vocabulary(&p, cwd, args);
    }
}

/// Runs one command and refuses every repository-scoped phrasing in it.
fn check_vocabulary(p: &Probe, cwd: &str, args: &[&str]) {
    let out = p.run(cwd, args);
    for phrase in REPO_SCOPED {
        assert!(
            !out.contains(phrase),
            "`gaff {}` says {phrase:?}, which scopes consent to a repository: {out}",
            args.join(" ")
        );
    }
    // The binary carried the false security claim too, in `trust
    // --help`, and this helper read only the list above. Four pages
    // were guarded and the fifth surface, the one the code owns, was
    // not. That is the shape this file exists to stop.
    for phrase in ABSOLUTE_GUARD_CLAIM {
        assert!(
            !out.contains(phrase),
            "`gaff {}` says {phrase:?}, which claims the guard stops a self-grant outright: {out}",
            args.join(" ")
        );
    }
}

#[test]
fn a_second_trust_in_one_directory_reports_the_existing_record() {
    let p = Probe::new("twice");
    p.run("repo", &["trust"]);
    let again = p.run("repo", &["trust"]);
    assert!(
        again.contains("was already trusted"),
        "a repeated trust did not report the existing record: {again}"
    );
    assert!(
        again.contains("consent is per directory"),
        "the repeated grant dropped the sentence that explains the miss: {again}"
    );
}

#[test]
fn asking_for_help_does_not_grant_consent() {
    // `trust` once ignored its arguments, so `gaff trust --help` wrote
    // the record and exited 0. A consent command must not grant consent
    // because the reader asked what it does.
    let p = Probe::new("help");
    let out = p.run("repo", &["trust", "--help"]);
    assert!(
        out.contains("Consent is per directory"),
        "trust --help did not explain the command: {out}"
    );
    assert!(
        !out.contains("may now run handlers"),
        "trust --help granted the consent it was asked to describe: {out}"
    );

    // The record must not exist, which is the claim that matters.
    let doctor = p.run("repo", &["doctor"]);
    assert!(
        doctor.contains("NOT trusted"),
        "trust --help left a record behind: {doctor}"
    );
}

#[test]
fn an_argument_trust_does_not_understand_is_refused() {
    let p = Probe::new("argument");
    let out = p.run("repo", &["trust", "nonsense"]);
    assert!(
        out.contains("takes no argument") && out.contains("nonsense"),
        "an unknown argument was not refused by name: {out}"
    );
    let doctor = p.run("repo", &["doctor"]);
    assert!(
        doctor.contains("NOT trusted"),
        "a refused trust still wrote the record: {doctor}"
    );
}

#[cfg(unix)]
#[test]
fn a_grant_is_not_reported_where_it_would_be_inert() {
    // `std::fs::write` creates at 0666 & ~umask. Under a umask of 002
    // that is group-writable, and `is_trusted` refuses a group-writable
    // list, so the grant did nothing while `trust` reported success on
    // every run. `doctor` is the check that matters: it reads the same
    // record through the same permission test.
    use std::os::unix::fs::PermissionsExt as _;

    let p = Probe::new("umask");
    let granted = p.run("repo", &["trust"]);
    assert!(granted.contains("may now run handlers"), "{granted}");

    let record = p.root.join("home/.config/gaff/trusted");
    let mode = std::fs::metadata(&record)
        .expect("the record exists after a grant")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "the record is readable or writable beyond its owner: {mode:o}"
    );

    let doctor = p.run("repo", &["doctor"]);
    assert!(
        doctor.contains("this directory is trusted"),
        "trust reported success and doctor disagrees, so the grant was inert: {doctor}"
    );
    assert!(
        !doctor.contains("writable by other users"),
        "gaff refused the record it wrote itself: {doctor}"
    );
}

#[cfg(unix)]
#[test]
fn an_existing_world_readable_record_is_narrowed() {
    // The upgrade path from 0.1.0, where `std::fs::write` under a umask
    // of 022 left the record at 0644. It passes the owner-only test, so
    // it is believed and adopted rather than refused, and only the
    // explicit chmod after the write narrows it. `mode` at creation
    // does nothing for a file that already exists.
    use std::os::unix::fs::PermissionsExt as _;

    let p = Probe::new("upgrade");
    let record = p.root.join("home/.config/gaff/trusted");
    std::fs::create_dir_all(record.parent().expect("the config directory"))
        .expect("the config directory is made");
    std::fs::write(&record, "").expect("the record writes");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644))
        .expect("the record is made world-readable");

    let out = p.run("repo", &["trust"]);
    assert!(out.contains("may now run handlers"), "{out}");
    let mode = std::fs::metadata(&record)
        .expect("the record exists")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "a record carried over from an earlier version stayed readable: {mode:o}"
    );
}

#[cfg(unix)]
#[test]
fn a_loose_record_is_refused_rather_than_adopted() {
    // Creating the record at 0600 fixed an inert grant, and opened a
    // worse hole: rewriting an existing group-writable list at 0600
    // kept its contents, so `trust` adopted every path another user had
    // appended and granted them command execution. `trust` must refuse
    // the loose list instead.
    use std::os::unix::fs::PermissionsExt as _;

    let p = Probe::new("loose");
    let record = p.root.join("home/.config/gaff/trusted");
    std::fs::create_dir_all(record.parent().expect("the config directory"))
        .expect("the config directory is made");
    let injected = p.root.join("repo/sub");
    std::fs::write(&record, format!("{}\n", injected.display())).expect("the record writes");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o664))
        .expect("the record is made group-writable");

    let out = p.run("repo", &["trust"]);
    assert!(
        !out.contains("may now run handlers"),
        "trust granted against a list it cannot believe: {out}"
    );
    assert!(
        out.contains("writable by other users"),
        "the refusal does not name the cause: {out}"
    );
    assert!(
        out.contains("chmod 600"),
        "the refusal names no remedy: {out}"
    );

    // The injected path must not have been adopted.
    let doctor = p.run("repo/sub", &["doctor"]);
    assert!(
        doctor.contains("NOT trusted"),
        "a path nobody granted became trusted: {doctor}"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_name_holding_a_line_break_is_refused() {
    // The record keeps one path to a line and `is_trusted` splits on
    // lines. Only `/` and NUL are forbidden in a name, so a name
    // carrying a line break wrote two records from one grant, and the
    // second path became trusted. An agent that can make a directory
    // picks that second path, then asks for a grant in the first.
    let p = Probe::new("newline");
    let victim = p.root.join("victim");
    std::fs::create_dir_all(&victim).expect("the victim directory is made");
    let hostile = p.root.join(format!("we\n{}", victim.display()));
    std::fs::create_dir_all(&hostile).expect("the hostile directory is made");

    let out = Command::new(env!("CARGO_BIN_EXE_gaff"))
        .arg("trust")
        .current_dir(&hostile)
        .env("HOME", p.root.join("home"))
        .env_remove("GAFF_STATE_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("gaff runs");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        !text.contains("may now run handlers"),
        "a name holding a line break was recorded: {text}"
    );
    assert!(
        text.contains("line break"),
        "the refusal does not name the cause: {text}"
    );

    // The path the name embedded must not have become trusted.
    let doctor = p.run("victim", &["doctor"]);
    assert!(
        doctor.contains("NOT trusted"),
        "one grant trusted a directory nobody named: {doctor}"
    );
}

#[test]
fn the_shipped_prose_agrees_with_the_binary() {
    // The man page and the configuration reference both describe this
    // command. A rename that reaches the code and not the prose leaves
    // a reader two answers, which is the defect these tests exist for.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for page in [
        "docs/man/gaff.1",
        "docs/configuration.md",
        "README.md",
        "SECURITY.md",
        "skills/gaff/SKILL.md",
        ".agents/skills/gaff-development/SKILL.md",
    ] {
        let text = std::fs::read_to_string(root.join(page)).expect("the page is readable");
        for phrase in REPO_SCOPED {
            assert!(
                !text.contains(phrase),
                "{page} says {phrase:?}, which scopes consent to a repository"
            );
        }
        for phrase in ABSOLUTE_GUARD_CLAIM {
            assert!(
                !text.contains(phrase),
                "{page} says {phrase:?}, which claims the guard stops a self-grant outright"
            );
        }
    }
}
