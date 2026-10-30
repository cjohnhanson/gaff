//! `docs/cli-reference.md` is generated from the man page, so the two
//! cannot drift. gaff has no clap definition to render from (see
//! `man_page_covers_the_cli.rs`), and the hand-written mdoc page is the
//! one source. pandoc's mdoc reader turns it into Markdown.
//!
//! This test fails when the committed page differs from what pandoc
//! renders. To rewrite the page:
//!
//!     UPDATE_CLI_REFERENCE=1 cargo test --test cli_reference_page
//!
//! The rendering depends on the pandoc version, so the version is
//! pinned here and in the CI step that installs it.

use std::process::Command;

const PANDOC_VERSION: &str = "3.7.0.2";

const MAN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/man/gaff.1");
const PAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/cli-reference.md");

const HEADER: &str = "<!-- metadata
title: gaff CLI reference
description: Every command and option of the gaff binary, generated from its man page
type: reference
-->

# gaff CLI reference
";

fn pandoc(args: &[&str]) -> String {
    let out = Command::new("pandoc")
        .args(args)
        .output()
        .unwrap_or_else(|e| {
            panic!("pandoc {PANDOC_VERSION} is needed to render the CLI reference page: {e}")
        });
    assert!(
        out.status.success(),
        "pandoc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("pandoc writes UTF-8")
}

fn rendered() -> String {
    let version = pandoc(&["--version"]);
    let first = version.lines().next().unwrap_or_default();
    assert_eq!(
        first,
        format!("pandoc {PANDOC_VERSION}"),
        "the page is rendered with pandoc {PANDOC_VERSION}; another version renders it differently"
    );
    // Raw HTML off, so the mdoc path and cross-reference macros come out
    // as plain text. Headings shift down one level under the page's H1.
    let body = pandoc(&[
        "--from=mdoc",
        "--to=gfm-raw_html",
        "--wrap=none",
        "--shift-heading-level-by=1",
        MAN,
    ]);
    format!("{HEADER}\n{}\n", body.trim_end())
}

#[test]
fn the_cli_reference_page_matches_the_man_page() {
    let want = rendered();
    if std::env::var_os("UPDATE_CLI_REFERENCE").is_some() {
        std::fs::write(PAGE, &want).expect("write the page");
        return;
    }
    let have = std::fs::read_to_string(PAGE).expect("read the page");
    assert!(
        have == want,
        "docs/cli-reference.md differs from docs/man/gaff.1. Rewrite it:\n  UPDATE_CLI_REFERENCE=1 cargo test --test cli_reference_page"
    );
}
