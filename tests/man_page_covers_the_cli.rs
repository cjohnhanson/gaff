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

/// The COMMANDS section, from its heading to the next one.
///
/// Every other lookup here reads this slice rather than the whole
/// page. An `.It Cm` entry belongs under ENVIRONMENT and under FILES
/// too, and scanning the whole page read those as command entries. The
/// page passed only because of where its sections happened to sit.
fn commands_section() -> &'static str {
    let start = MAN
        .find("\n.Sh COMMANDS\n")
        .expect("docs/man/gaff.1 has a COMMANDS section");
    let rest = &MAN[start + 1..];
    rest[1..].find("\n.Sh ").map_or(rest, |end| &rest[..=end])
}

/// Command names opening an `.It Cm` entry in the COMMANDS section.
/// Only that form counts. A mention in prose is not documentation, and
/// treating it as such once let a deleted entry pass this test.
fn documented() -> Vec<&'static str> {
    commands_section()
        .lines()
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
    // Only the COMMANDS section. An `.It Cm` entry belongs under
    // ENVIRONMENT and under FILES too, and those name a variable or a
    // path rather than a command.
    for name in documented() {
        assert!(
            COMMANDS.contains(&name),
            "docs/man/gaff.1 documents `{name}` under COMMANDS, which no command dispatches"
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

/// The `reviews check` prose, from the sign-off paragraph to the end
/// of the `reviews` entry.
///
/// A reviewer deleted this whole block and every test here stayed
/// green, then falsified its evidence floor and they stayed green
/// again. The page described a gate that no test read, which is the
/// same class of defect the gate itself exists to catch.
fn reviews_check_prose() -> &'static str {
    let section = commands_section();
    let start = section
        .find(".Cm reviews check")
        .expect("docs/man/gaff.1 describes `reviews check`");
    let rest = &section[start..];
    rest.find("\n.It Cm ").map_or(rest, |end| &rest[..end])
}

#[test]
fn the_evidence_floor_in_the_man_page_matches_the_code() {
    let want = format!("fewer than {} words", gaff::reviewnote::EVIDENCE_FLOOR);
    assert!(
        reviews_check_prose().contains(&want),
        "docs/man/gaff.1 states an evidence floor that is not `{want}`"
    );
}

#[test]
fn every_fault_that_refuses_a_push_is_documented() {
    // One phrase for each `reviewnote::Missing` variant, in the order
    // the page lists them. A variant added with no phrase here is not
    // caught. A phrase deleted from the page is.
    const FAULTS: [&str; 7] = [
        "a declared review with no sign-off",
        "a verdict of",
        "a sign-off naming another commit",
        "two sign-offs for one review",
        "evidence of fewer than",
        "a sha of fewer than 7 characters, or one that is not hex",
        "no note at all",
    ];
    let prose = reviews_check_prose();
    for fault in FAULTS {
        assert!(
            prose.contains(fault),
            "docs/man/gaff.1 no longer names the fault `{fault}`"
        );
    }
}

#[test]
fn the_man_page_documents_no_flag_on_reviews_check() {
    // `--head` is removed. `gaff ci` substitutes the pull request head
    // into the ref line it synthesizes, so one mechanism carries it.
    // The page claimed `ci` passed a flag that no caller ever passed.
    assert!(
        !reviews_check_prose().contains("-head"),
        "docs/man/gaff.1 documents a --head flag that `reviews check` does not take"
    );
}

#[test]
fn the_sha_floor_in_the_man_page_matches_the_code() {
    let want = format!(
        "a sha of fewer than {} characters",
        gaff::reviewnote::SHA_FLOOR
    );
    assert!(
        reviews_check_prose().contains(&want),
        "docs/man/gaff.1 states a sha floor that is not `{want}`"
    );
}
