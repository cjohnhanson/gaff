//! The docs are tests.
//!
//! `console_examples` runs every `console` block in README.md and
//! docs/*.md against the built binary with trycmd, and compares the
//! printed output with the block. Each page runs top to bottom in its own
//! scratch directory, with `HOME` and the XDG directories in that
//! directory, so gaff reads an empty user config, an empty trust record,
//! and fresh state. Every command a page runs goes through a wrapper that
//! starts it with a cleared environment: a `GAFF_SESSION_ID`,
//! `GAFF_STATE_DIR`, or `GIT_DIR` inherited from the shell that ran
//! `cargo test` cannot change what a page prints.
//!
//! A page can have fixtures in tests/docs/<page>/: `setup.md` holds
//! `console` blocks that run before the page, for examples that assume
//! earlier state; `work/` is copied into the working directory and
//! `home/` into `HOME`; `env` holds `KEY=value` lines added to every
//! command the page runs, for a page whose examples run inside an agent
//! session.
//!
//! `cli_reference_matches_help` checks docs/cli-reference.md: the text
//! between its generated markers is the help of every command that has
//! its own, read from the built binary. gaff parses its arguments by hand
//! (src/cli.rs says why), so `gaff --help` is the source: its Commands
//! list names the commands, and a command has its own help when
//! `gaff <command> --help` succeeds and prints `gaff <command>` first.
//!
//! Write the real output into the pages, then read the diff:
//! `TRYCMD=overwrite cargo test --test docs` for the examples,
//! `SNAPSHOTS=overwrite cargo test --test docs` for the CLI reference.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_gaff");
const BIN_SHORT: &str = env!("CARGO_BIN_EXE_gaffr");
const BEGIN: &str = "<!-- Begin generated CLI help: cargo test --test docs -->";
const END: &str = "<!-- End generated CLI help -->";
/// The width every example and every help run sees.
const COLUMNS: &str = "100";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// README.md and every page in docs/, in name order.
fn pages() -> Vec<PathBuf> {
    let mut pages: Vec<PathBuf> = std::fs::read_dir(root().join("docs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    pages.sort();
    pages.insert(0, root().join("README.md"));
    pages
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let guard = tempfile::tempdir().unwrap();
    // Canonical, so a path gaff prints from its working directory is the
    // path trycmd writes as `[CWD]`.
    let path = guard.path().canonicalize().unwrap();
    (guard, path)
}

#[test]
fn console_examples() {
    let (_guard, scratch) = scratch();
    let mut failed = Vec::new();
    for page in pages() {
        if std::panic::catch_unwind(|| run_page(&scratch, &page)).is_err() {
            failed.push(page.strip_prefix(root()).unwrap().display().to_string());
        }
    }
    assert!(
        failed.is_empty(),
        "examples differ from the binary in: {failed:?}"
    );
}

/// Run one page's `console` blocks in `scratch/<stem>/`.
fn run_page(scratch: &Path, page: &Path) {
    let stem = page.file_stem().unwrap().to_string_lossy().into_owned();
    let fixtures = root().join("tests/docs").join(&stem);
    let dir = scratch.join(&stem);
    // trycmd runs `page.md` in `page.in/`.
    let work = dir.join("page.in");
    let home = dir.join("home");
    let bin = dir.join("bin");
    for (from, to) in [
        (fixtures.join("work"), &work),
        (fixtures.join("home"), &home),
    ] {
        if from.is_dir() {
            copy_dir(&from, to);
        } else {
            std::fs::create_dir_all(to).unwrap();
        }
    }
    std::fs::create_dir_all(&bin).unwrap();
    let setup = std::fs::read_to_string(fixtures.join("setup.md")).unwrap_or_default();
    let extra = page_env(&fixtures);
    let case = dir.join("page.md");
    let body = std::fs::read_to_string(page).unwrap();
    let text = format!("{setup}{body}");
    std::fs::write(&case, &text).unwrap();

    let home_var = |sub: &str| home.join(sub).display().to_string();
    let cases = trycmd::TestCases::new();
    // trycmd skips, and does not fail, a command it cannot resolve, and it
    // resolves an unregistered crate binary to the binary itself, outside
    // the wrapper. So every command a page runs is registered here, from
    // the blocks trycmd runs, or the page fails.
    for name in commands(page, &text) {
        let target = match name.as_str() {
            "gaff" => PathBuf::from(BIN),
            "gaffr" => PathBuf::from(BIN_SHORT),
            _ => std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(&name))
                .find(|p| p.is_file())
                .unwrap_or_else(|| {
                    panic!("{} runs `{name}`, which is not on PATH", page.display())
                }),
        };
        cases.register_bin(name.clone(), write_wrapper(&bin, &name, &target, &extra));
    }
    cases
        .env("HOME", home.display().to_string())
        .env("XDG_CONFIG_HOME", home_var(".config"))
        .env("XDG_STATE_HOME", home_var(".local/state"))
        .env("XDG_DATA_HOME", home_var(".local/share"))
        .case(&case)
        .run();

    // TRYCMD=overwrite rewrote the scratch copy. Carry the page's part
    // back to the page; the setup part stays in tests/docs.
    if std::env::var_os("TRYCMD").is_some_and(|m| m == "overwrite") {
        let written = std::fs::read_to_string(&case).unwrap();
        let body = written.strip_prefix(&setup).unwrap_or_else(|| {
            panic!(
                "the setup for {} changed; run it again after updating setup.md",
                page.display()
            )
        });
        std::fs::write(page, body).unwrap();
    }
}

/// The `KEY=value` lines of a page's `env` fixture.
fn page_env(fixtures: &Path) -> Vec<String> {
    std::fs::read_to_string(fixtures.join("env"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (key, value) = l
                .split_once('=')
                .unwrap_or_else(|| panic!("{}: not KEY=value: {l}", fixtures.display()));
            assert!(
                key.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') && !value.contains('\''),
                "{}: an env line is KEY=value, with no quote in the value: {l}",
                fixtures.display()
            );
            format!("{key}='{value}'")
        })
        .collect()
}

/// A `/bin/sh` wrapper that runs `target` with a cleared environment:
/// the page's `HOME` and XDG directories, `PATH`, a fixed terminal, and
/// the page's `env` fixture. trycmd runs it with the page's working
/// directory.
fn write_wrapper(bin: &Path, name: &str, target: &Path, extra: &[String]) -> PathBuf {
    let wrapper = bin.join(name);
    // The wrapper quotes the path in single quotes.
    assert!(
        !target.to_string_lossy().contains('\''),
        "cannot wrap a path holding a single quote: {}",
        target.display()
    );
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n\
             exec env -i HOME=\"$HOME\" XDG_CONFIG_HOME=\"$XDG_CONFIG_HOME\" \
             XDG_STATE_HOME=\"$XDG_STATE_HOME\" XDG_DATA_HOME=\"$XDG_DATA_HOME\" \
             PATH=\"$PATH\" NO_COLOR=1 TERM=dumb COLUMNS={COLUMNS} GIT_CONFIG_NOSYSTEM=1 \
             {} '{}' \"$@\"\n",
            extra.join(" "),
            target.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    wrapper
}

/// The lines of every block trycmd runs, read by trycmd 1.2's own rules
/// (`parse_trycmd` in its schema.rs): a fence with no info string, or
/// one whose language is `console` or `trycmd`, runs; the block ends at
/// the first line that starts with the opening backticks.
///
/// Panics on a `console,ignore` or `trycmd,ignore` block: trycmd skips
/// it, so its commands would never be checked.
fn run_lines<'a>(page: &Path, text: &'a str) -> Vec<&'a str> {
    let mut lines = Vec::new();
    // The open fence: its backticks, and whether trycmd runs its body.
    let mut fence: Option<(&str, bool)> = None;
    for line in text.lines() {
        if let Some((marker, runs)) = fence {
            if line.starts_with(marker) {
                fence = None;
            } else if runs {
                lines.push(line);
            }
            continue;
        }
        let ticks = line.len() - line.trim_start_matches('`').len();
        if ticks < 3 {
            continue;
        }
        let info = line[ticks..].trim();
        let mut parts = info.split(',');
        let lang = parts.next().unwrap_or_default();
        let runs = info.is_empty() || lang == "console" || lang == "trycmd";
        assert!(
            !(runs && parts.any(|p| p == "ignore")),
            "{}: `{info}` hides its commands from the docs test",
            page.display()
        );
        fence = Some((&line[..ticks], runs));
    }
    lines
}

/// The command word of every `$ ` line in the blocks trycmd runs.
///
/// Panics on an output line holding `\`: trycmd reads `\` in real output
/// as `/`, so such a line can never match the binary.
fn commands(page: &Path, text: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for line in run_lines(page, text) {
        if let Some(cmd) = line.strip_prefix("$ ") {
            if let Some(word) = cmd.split_whitespace().find(|w| !w.contains('=')) {
                names.insert(word.to_string());
            }
        } else if !line.starts_with("> ") {
            // `> ` continues a command; every other line is output.
            assert!(
                !line.contains('\\'),
                "{}: trycmd cannot match output holding a backslash; choose an example without one: {line}",
                page.display()
            );
        }
    }
    names
}

/// Run the built binary with `args` in an empty directory, with an empty
/// `HOME` and nothing on stdin. Returns the exit status and stdout.
fn run_isolated(args: &[&str]) -> (bool, String) {
    let (_guard, scratch) = scratch();
    let home = scratch.join("home");
    let work = scratch.join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let out = Command::new(BIN)
        .args(args)
        .current_dir(&work)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("COLUMNS", COLUMNS)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap())
}

/// The commands `gaff --help` lists under `Commands:`, in its order.
fn listed_commands() -> Vec<String> {
    let (ok, help) = run_isolated(&["--help"]);
    assert!(ok, "gaff --help failed");
    let list = help
        .split_once("\nCommands:\n")
        .expect("gaff --help has a Commands section")
        .1;
    let mut names: Vec<String> = Vec::new();
    for line in list.lines().take_while(|l| !l.is_empty()) {
        // An entry opens with two spaces and the command; a wrapped
        // description line is indented further.
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if rest.starts_with(' ') {
            continue;
        }
        let name = rest.split_whitespace().next().unwrap().to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Whether `gaff <command> --help` prints help of its own. A command
/// that ignores its arguments (`doctor`, `prime`) succeeds with its usual
/// output, so success alone is not help.
fn own_help(command: &str) -> Option<String> {
    let (ok, out) = run_isolated(&[command, "--help"]);
    let title = format!("gaff {command}");
    (ok && out.lines().next() == Some(title.as_str())).then_some(out)
}

/// The command paths with help of their own: `[]` for `gaff --help`,
/// then each listed command that answers `--help`.
fn help_paths() -> Vec<Vec<String>> {
    std::iter::once(Vec::new())
        .chain(
            listed_commands()
                .into_iter()
                .filter(|c| own_help(c).is_some())
                .map(|c| vec![c]),
        )
        .collect()
}

/// The generated part of the CLI reference: one section per command
/// with help of its own, as a `console` transcript. A `text` fence would
/// do, but help lines open with `gaff`, and the fence lint shared by the
/// eng.rip tools (`scripts/family/check-fences.py` in cjohnhanson/eng.rip)
/// reads such a line in a `text` fence as a command; the transcript also
/// runs in `console_examples`.
fn generated_reference() -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for path in help_paths() {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let (ok, help) = run_isolated(&args);
        assert!(ok, "gaff {} failed", args.join(" "));
        let name = std::iter::once("gaff")
            .chain(path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        write!(
            text,
            "\n## {name}\n\n```console\n$ {name} --help\n{help}\n```\n"
        )
        .unwrap();
    }
    text
}

#[test]
fn cli_reference_matches_help() {
    let path = root().join("docs/cli-reference.md");
    let page = std::fs::read_to_string(&path).unwrap();
    let begin = page
        .find(BEGIN)
        .expect("docs/cli-reference.md lacks the begin marker")
        + BEGIN.len();
    let end = page
        .find(END)
        .expect("docs/cli-reference.md lacks the end marker");
    let generated = format!(
        "{}\n{}\n{}",
        &page[..begin],
        generated_reference(),
        &page[end..]
    );
    snapbox::Assert::new()
        .action_env("SNAPSHOTS")
        .eq(generated, snapbox::Data::read_from(&path, None).raw());
}

/// The reference reads its command list from `gaff --help`. An empty or
/// short list would generate a reference that passes while covering
/// nothing.
#[test]
fn the_help_lists_the_commands_and_trust_has_its_own_help() {
    let listed = listed_commands();
    for name in [
        "hook", "remind", "init", "trust", "doctor", "prime", "reviews",
    ] {
        assert!(listed.iter().any(|c| c == name), "{name} not in {listed:?}");
    }
    assert!(own_help("trust").is_some());
    // These succeed on `--help` and print their usual output instead.
    assert!(own_help("doctor").is_none());
    assert!(own_help("prime").is_none());
    assert_eq!(help_paths(), [vec![], vec!["trust".to_string()]]);
    assert_eq!(
        listed,
        [
            "hook", "remind", "status", "init", "githook", "check", "trust", "allow", "doctor",
            "profile", "run", "log", "docs", "ci", "reviews", "prime"
        ]
    );
    // README.md runs first, then every page in docs/.
    let pages = pages();
    assert!(
        pages[0].ends_with("README.md") && pages.len() > 4,
        "{pages:?}"
    );
}

/// The harness itself: a page whose output differs from the binary fails,
/// and a page that matches passes.
#[test]
fn a_wrong_example_fails_and_a_right_one_passes() {
    if std::env::var_os("TRYCMD").is_some() {
        return; // overwrite mode would rewrite the wrong page and pass
    }
    let (_guard, scratch) = scratch();
    let right = scratch.join("right.md");
    std::fs::write(
        &right,
        "```console\n$ gaff check\nno config at .gaff/gaff.yml. Nothing to validate.\n\n```\n",
    )
    .unwrap();
    run_page(&scratch, &right);
    let wrong = scratch.join("wrong.md");
    std::fs::write(&wrong, "```console\n$ gaff check\nconfig ok\n\n```\n").unwrap();
    assert!(std::panic::catch_unwind(|| run_page(&scratch, &wrong)).is_err());
}

/// An inherited session or state directory does not reach a page: the
/// wrapper clears the environment.
#[test]
fn an_inherited_gaff_variable_does_not_reach_a_page() {
    if std::env::var_os("TRYCMD").is_some() {
        return;
    }
    let (_guard, scratch) = scratch();
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let wrapper = write_wrapper(&bin, "gaff", Path::new(BIN), &[]);
    let out = Command::new(&wrapper)
        .arg("status")
        .current_dir(&scratch)
        .env("HOME", &scratch)
        .env("GAFF_SESSION_ID", "leaked")
        .env("CLAUDE_CODE_SESSION_ID", "leaked")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "status found a session: {stderr}");
    assert!(stderr.contains("no session"), "{stderr}");
}

#[test]
fn commands_lists_what_trycmd_runs() {
    let page = "```sh\nls\n```\n\n````markdown\n```console\n$ inner\n```\n````\n\n```console\n$ A=1 gaff status\n\n$ git init\nok\n```\n\n```\n$ bare\n```\n\n```trycmd\n$ tagged\n```\n";
    let names: Vec<String> = commands(Path::new("p.md"), page).into_iter().collect();
    assert_eq!(names, ["bare", "gaff", "git", "tagged"]);
}

/// Only the opening backtick run closes a fence. A shorter run inside a
/// longer fence is text, so the `$ y` after it is not a command.
#[test]
fn a_shorter_fence_inside_a_longer_one_does_not_close_it() {
    let page = "````markdown\n```console\nx\n```\n$ y\n````\n";
    assert!(commands(Path::new("p.md"), page).is_empty());
}

/// Every flag the help names appears in the man page, which the CLI
/// reference sends readers to for each command's options.
#[test]
fn the_man_page_names_every_flag_the_help_names() {
    let (ok, help) = run_isolated(&["--help"]);
    assert!(ok);
    let man = std::fs::read_to_string(root().join("docs/man/gaff.1")).unwrap();
    let flags: std::collections::BTreeSet<&str> = help
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter_map(|w| w.strip_prefix("--"))
        .filter(|f| !f.is_empty() && *f != "help" && *f != "version")
        .collect();
    assert!(
        flags.contains("session") && flags.contains("clear"),
        "{flags:?}"
    );
    for flag in flags {
        assert!(
            man.contains(&format!("Fl -{flag}")),
            "docs/man/gaff.1 does not name --{flag}"
        );
    }
    assert!(
        man.contains(".It Cm docs search"),
        "the man page omits docs search"
    );
}

#[test]
fn a_page_running_an_unknown_command_fails() {
    let (_guard, scratch) = scratch();
    let page = scratch.join("unknown.md");
    std::fs::write(&page, "```console\n$ no-such-command-in-path\n```\n").unwrap();
    let err = std::panic::catch_unwind(|| run_page(&scratch, &page)).unwrap_err();
    let message = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("not on PATH"), "{message}");
}

/// Every file under `dir`, depth first.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The files in a page's `work/` and `home/` fixtures that the page does
/// not show. A reader following the page has only what it shows, so each
/// such file must appear in a fence named for its kind: `yaml` for
/// `.yml`, `markdown` for `.md`. Returns how many files it checked and
/// the ones missing.
fn unshown_fixtures(text: &str, fixtures: &Path) -> (usize, Vec<PathBuf>) {
    let mut files = Vec::new();
    files_under(&fixtures.join("work"), &mut files);
    files_under(&fixtures.join("home"), &mut files);
    let checked = files.len();
    let missing = files
        .into_iter()
        .filter(|file| {
            let lang = match file.extension().and_then(|e| e.to_str()) {
                Some("yml") => "yaml",
                Some("md") => "markdown",
                _ => panic!("{}: no fence language for this fixture", file.display()),
            };
            let body = std::fs::read_to_string(file).unwrap();
            !text.contains(&format!("```{lang}\n{body}```"))
        })
        .collect();
    (checked, missing)
}

/// The fixtures are the files a page's examples run against, so the page
/// must show each one.
#[test]
fn every_fixture_file_is_shown_on_its_page() {
    let mut checked = 0;
    for page in pages() {
        let stem = page.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&page).unwrap();
        let (count, missing) = unshown_fixtures(&text, &root().join("tests/docs").join(&stem));
        assert!(
            missing.is_empty(),
            "{} does not show {missing:?}",
            page.display()
        );
        checked += count;
    }
    assert!(checked > 0, "no fixture file was checked");
}

/// A section file the page never shows is caught, and so is a config
/// shown with different text.
#[test]
fn a_fixture_the_page_does_not_show_is_reported() {
    let (_guard, scratch) = scratch();
    let section = scratch.join("work/.gaff/sections/a.md");
    std::fs::create_dir_all(section.parent().unwrap()).unwrap();
    std::fs::write(&section, "Keep this.\n").unwrap();
    std::fs::write(scratch.join("work/.gaff/gaff.yml"), "sections: []\n").unwrap();
    let page = "```yaml\nsections: []\n```\n";
    let (checked, missing) = unshown_fixtures(page, &scratch);
    assert_eq!(checked, 2);
    assert_eq!(missing, [section]);
    let page = "```yaml\nsections: [x]\n```\n\n```markdown\nKeep this.\n```\n";
    let (_, missing) = unshown_fixtures(page, &scratch);
    assert_eq!(missing, [scratch.join("work/.gaff/gaff.yml")]);
}

#[test]
#[should_panic(expected = "hides its commands")]
fn an_ignored_console_block_is_refused() {
    commands(Path::new("p.md"), "```console,ignore\n$ gaff status\n```\n");
}

#[test]
#[should_panic(expected = "backslash")]
fn output_with_a_backslash_is_refused() {
    commands(
        Path::new("p.md"),
        "```console\n$ gaff docs\n\"a\\\"b\"\n```\n",
    );
}
