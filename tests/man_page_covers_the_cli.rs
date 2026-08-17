//! The man page is hand-written mdoc, not generated.
//!
//! The other tools in this suite derive their pages from clap. gaff
//! parses arguments by hand, deliberately, because clap exits 2 on a
//! usage error and gaff reserves 2 for a guard refusing a tool call.
//! So no `clap::Command` exists to render from.
//!
//! Generation exists to stop documentation drifting from the CLI. This
//! test buys the same thing: a command that dispatches but carries no
//! man entry fails here, and so does an entry for a command that no
//! longer exists.

/// Every command `run` dispatches, mirroring `COMMANDS` in `src/cli.rs`.
/// That constant is private, and a second copy is the point: a command
/// added there without a man entry fails this test.
const COMMANDS: [&str; 15] = [
    "hook", "githook", "remind", "allow", "status", "init", "check", "doctor", "trust", "profile",
    "log", "docs", "prime", "ci", "reviews",
];

const MAN: &str = include_str!("../docs/man/gaff.1");

/// Command names opening an `.It Cm` entry in the COMMANDS section.
/// Only that form counts. A mention in prose is not documentation, and
/// treating it as such once let a deleted entry pass this test.
fn documented() -> Vec<&'static str> {
    MAN.lines()
        .filter_map(|l| l.strip_prefix(".It Cm "))
        .filter_map(|rest| rest.split_whitespace().next())
        .collect()
}

#[test]
fn every_command_has_a_man_entry() {
    let entries = documented();
    for cmd in COMMANDS {
        assert!(
            entries.contains(&cmd),
            "`{cmd}` dispatches but opens no `.It Cm {cmd}` entry in docs/man/gaff.1"
        );
    }
}

#[test]
fn the_man_page_documents_no_command_that_vanished() {
    // Collect the .It Cm entries and check each names a real command.
    for line in MAN.lines() {
        let Some(rest) = line.strip_prefix(".It Cm ") else {
            continue;
        };
        let name = rest.split_whitespace().next().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        assert!(
            COMMANDS.contains(&name),
            "docs/man/gaff.1 documents `{name}`, which no command dispatches"
        );
    }
}

#[test]
fn the_command_list_matches_the_source() {
    // src/cli.rs owns the real list. A drift between it and the copy
    // above means this test is checking a stale set.
    let src = include_str!("../src/cli.rs");
    // Locate the declaration by name, never by the length in its type.
    // A search string carrying the count fails on the search itself
    // when a command is added, and the panic then names the search
    // rather than the drift. That happened when `reviews` landed.
    let start = src
        .find("const COMMANDS: [&str;")
        .expect("src/cli.rs declares COMMANDS");
    let end = src[start..].find("];").expect("COMMANDS is terminated") + start;
    let block = &src[start..end];
    for cmd in COMMANDS {
        assert!(
            block.contains(&format!("\"{cmd}\"")),
            "`{cmd}` is in this test's copy but not in src/cli.rs COMMANDS"
        );
    }
    let count = block.matches('"').count() / 2;
    assert_eq!(
        count,
        COMMANDS.len(),
        "src/cli.rs lists {count} commands, this test lists {}",
        COMMANDS.len()
    );
}
