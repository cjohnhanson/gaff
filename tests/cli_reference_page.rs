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
//! pinned here and in the CI steps that install it. Without that
//! pandoc on PATH the test skips, as the node tests do, so a
//! contributor, the nix build, and a crates.io consumer can still run
//! the suite. In CI (where `CI` is set) a missing or different pandoc
//! fails the test instead, so the check cannot go quiet there.

use std::process::Command;

const PANDOC_VERSION: &str = "3.7.0.2";

/// The pinned pandoc's version line, or why it is not usable.
fn pinned_pandoc() -> Result<(), String> {
    let out = Command::new("pandoc")
        .arg("--version")
        .output()
        .map_err(|e| format!("pandoc is not on PATH: {e}"))?;
    let version = String::from_utf8_lossy(&out.stdout);
    let first = version.lines().next().unwrap_or_default();
    if first == format!("pandoc {PANDOC_VERSION}") {
        Ok(())
    } else {
        Err(format!(
            "found `{first}`, and the page is rendered with pandoc {PANDOC_VERSION}"
        ))
    }
}

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
        .expect("run pandoc");
    assert!(
        out.status.success(),
        "pandoc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("pandoc writes UTF-8")
}

fn rendered() -> String {
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

/// Whether a missing pinned pandoc fails the check rather than skipping
/// it: in CI, and when asked to rewrite the page.
const fn missing_pandoc_fails(update: bool, ci: bool) -> bool {
    update || ci
}

#[test]
fn a_missing_pandoc_fails_in_ci_and_on_update_and_skips_otherwise() {
    assert!(missing_pandoc_fails(false, true));
    assert!(missing_pandoc_fails(true, false));
    assert!(missing_pandoc_fails(true, true));
    assert!(!missing_pandoc_fails(false, false));
}

#[test]
fn the_cli_reference_page_matches_the_man_page() {
    let update = std::env::var_os("UPDATE_CLI_REFERENCE").is_some();
    if let Err(why) = pinned_pandoc() {
        assert!(
            !missing_pandoc_fails(update, std::env::var_os("CI").is_some()),
            "pandoc {PANDOC_VERSION} is needed to render the CLI reference page: {why}. \
             Install it from https://github.com/jgm/pandoc/releases/tag/{PANDOC_VERSION}"
        );
        eprintln!("skipping the CLI reference check: {why}");
        return;
    }
    let want = rendered();
    if update {
        std::fs::write(PAGE, &want).expect("write the page");
        return;
    }
    let have = std::fs::read_to_string(PAGE).expect("read the page");
    assert!(
        have == want,
        "docs/cli-reference.md differs from docs/man/gaff.1. Rewrite it:\n  UPDATE_CLI_REFERENCE=1 cargo test --test cli_reference_page"
    );
}
