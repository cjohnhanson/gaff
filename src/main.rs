//! gaff — a context-lifecycle handler for coding agents.
//!
//! The exit-code rule: every gaff invocation exits 0 or 1. It never
//! exits 2. The agent side treats exit 2 as the blocking code, and no
//! gaff failure may block a session. This covers a config typo, an
//! unwritable state directory, and a bad flag.
//!
//! This file parses the arguments by hand for that reason. clap exits 2
//! on a usage error.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use gaff::config::{self, Loaded};
use gaff::engine;
use gaff::event::Envelope;
use gaff::state::{resolve_root, Store};
use gaff::{docs, init};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => run_hook(),
        Some("remind") => run_remind(&args[1..]),
        Some("status") => run_status(&args[1..]),
        Some("init") => run_init(&args[1..]),
        Some("check") => run_check(),
        Some("doctor") => run_doctor(),
        Some("docs") => run_docs(&args[1..]),
        Some("--version" | "-V" | "version") => {
            println!("gaff {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => fail(&format!(
            "unknown command `{other}` (available: hook, remind, status, init, check, doctor, docs)"
        )),
        None => fail("usage: gaff <hook|remind|status|init|check|doctor|docs>"),
    }
}

const USAGE: &str = "gaff — a context-lifecycle handler for coding agents

Usage: gaff <command> [options]

Commands:
  hook             Handle one hook event from stdin (the harness calls this)
  remind <text>    Schedule a one-shot reminder N tool calls ahead
  status           Show counters, pending entries, and one-shots
  init             Register the hooks in .claude/settings.local.json
  check            Validate .gaff/gaff.yml
  doctor           Show what is live in this clone
  docs [page]      Print the bundled documentation

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Every gaff invocation exits 0 or 1, never 2. See man gaff for details.";

fn fail(msg: &str) -> ExitCode {
    eprintln!("gaff: {msg}");
    ExitCode::FAILURE
}

fn resolve_store() -> Option<Store> {
    let cwd = std::env::current_dir().ok()?;
    let root = resolve_root(
        std::env::var("GAFF_STATE_DIR").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        &cwd,
    )?;
    Some(Store::new(root))
}

fn session_from(flag: Option<&str>) -> Option<String> {
    flag.map(ToString::to_string)
        .or_else(|| std::env::var("CLAUDE_CODE_SESSION_ID").ok())
}

/// Read one hook payload from stdin. Write the response to stdout.
///
/// Unreadable stdin and unparseable stdin exit 1. Claude Code treats
/// exit 1 as a non-blocking error. Every later failure degrades to a
/// silent passthrough at exit 0.
fn run_hook() -> ExitCode {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return fail("cannot read stdin");
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&input) else {
        return fail("invalid JSON on stdin");
    };
    let envelope = Envelope::from_claude_code(payload);

    let Some(store) = resolve_store() else {
        eprintln!("gaff: no state directory. Set GAFF_STATE_DIR or HOME. Passing through.");
        return ExitCode::SUCCESS;
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = match config::load(&cwd) {
        Loaded::Ok(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => {
            eprintln!(
                "gaff: the config {} is not valid: {err}. Continuing without reminders.",
                config::CONFIG_PATH
            );
            store.mark_degraded();
            config::Config::default()
        }
    };

    if let Some(context) = engine::handle(&envelope, &cfg, &store, &cwd.join(".gaff")) {
        println!(
            "{}",
            json!({
                "hookSpecificOutput": {
                    "hookEventName": envelope.event,
                    "additionalContext": context,
                }
            })
        );
    }
    ExitCode::SUCCESS
}

/// `gaff remind <text> --after <N> [--id <id>] [--session <sid>]`
///
/// Schedule a one-shot reminder N tool calls into the session's future.
fn run_remind(args: &[String]) -> ExitCode {
    let mut text: Option<String> = None;
    let mut after: Option<u64> = None;
    let mut id: Option<String> = None;
    let mut session_flag: Option<String> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--after" => match it.next().map(|v| v.parse::<u64>()) {
                Some(Ok(n)) => after = Some(n),
                _ => return fail("--after requires a non-negative integer"),
            },
            "--id" => match it.next() {
                Some(v) => id = Some(v.clone()),
                None => return fail("--id requires a value"),
            },
            "--session" => match it.next() {
                Some(v) => session_flag = Some(v.clone()),
                None => return fail("--session requires a value"),
            },
            other if text.is_none() && !other.starts_with("--") => text = Some(other.to_string()),
            other => return fail(&format!("unexpected argument `{other}`")),
        }
    }

    let Some(text) = text else {
        return fail("usage: gaff remind <text> --after <N> [--id <id>] [--session <sid>]");
    };
    let Some(after) = after else {
        return fail("--after is required");
    };
    let Some(session) = session_from(session_flag.as_deref()) else {
        return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID.");
    };
    let Some(store) = resolve_store() else {
        return fail("no state directory. Set GAFF_STATE_DIR or HOME.");
    };

    let counts = store.counts(&session);
    let id = id.unwrap_or_else(|| format!("r{}", store.oneshots(&session).len() + 1));
    let at = counts.tool_calls + after;
    match store.write_oneshot(&session, &id, after, at, &text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("cannot write the reminder: {e}")),
    }
}

/// `gaff status [--session <sid>]` — print the session counters as JSON.
fn run_status(args: &[String]) -> ExitCode {
    let mut session_flag: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" => match it.next() {
                Some(v) => session_flag = Some(v.clone()),
                None => return fail("--session requires a value"),
            },
            other => return fail(&format!("unexpected argument `{other}`")),
        }
    }
    let Some(session) = session_from(session_flag.as_deref()) else {
        return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID.");
    };
    let Some(store) = resolve_store() else {
        return fail("no state directory. Set GAFF_STATE_DIR or HOME.");
    };
    let counts = store.counts(&session);
    let oneshots: Vec<_> = store
        .oneshots(&session)
        .into_iter()
        .map(|s| json!({"at": s.at, "fired": store.is_fired(&session, &s.id), "id": s.id}))
        .collect();
    println!(
        "{}",
        json!({
            "oneshots": oneshots,
            "pending": store.pendings(&session),
            "prompts": counts.prompts,
            "tool_calls": counts.tool_calls,
        })
    );
    ExitCode::SUCCESS
}

/// `gaff init [--uninstall] [--command <cmd>]` — register the hook
/// entries in `.claude/settings.local.json`, or remove them.
fn run_init(args: &[String]) -> ExitCode {
    let mut uninstall = false;
    let mut command = "gaff hook".to_string();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--uninstall" => uninstall = true,
            "--command" => match it.next() {
                Some(v) => command.clone_from(v),
                None => return fail("--command requires a value"),
            },
            other => return fail(&format!("unexpected argument `{other}`")),
        }
    }
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let result = if uninstall {
        init::uninstall(&cwd, &command)
    } else {
        init::install(&cwd, &command)
    };
    match result {
        Ok(init::Outcome::Changed) => {
            let verb = if uninstall {
                "removed from"
            } else {
                "registered in"
            };
            println!("gaff hooks {verb} {}", init::SETTINGS_PATH);
            ExitCode::SUCCESS
        }
        Ok(init::Outcome::Unchanged) => {
            println!("already up to date: {}", init::SETTINGS_PATH);
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e.to_string()),
    }
}

/// `gaff check` — validate `.gaff/gaff.yml`. Exit 1 for an invalid
/// config. This command is the one place where a loud failure is correct.
fn run_check() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let cfg = match config::load(&cwd) {
        Loaded::Absent => {
            println!("no config at {}. Nothing to validate.", config::CONFIG_PATH);
            return ExitCode::SUCCESS;
        }
        Loaded::Broken(err) => return fail(&format!("{}: {err}", config::CONFIG_PATH)),
        Loaded::Ok(cfg) => cfg,
    };

    let mut problems = Vec::new();
    let mut names = std::collections::HashSet::new();
    let cadence_ok =
        |e: &config::Every| e.tool_calls.is_none_or(|n| n > 0) && e.prompts.is_none_or(|n| n > 0);
    // A name keys the pending, cursor, oneshot, and fired files. A `/`
    // or a `\` would make the name escape the session directory, so the
    // name must be a plain file component.
    let name_ok = |n: &str| !n.is_empty() && !n.contains(['/', '\\']);

    if cfg.max_inject_bytes == 0 {
        problems.push("max_inject_bytes is 0, so every flush is empty".to_string());
    }

    for r in &cfg.reminders {
        if !names.insert(r.name.clone()) {
            problems.push(format!("duplicate name `{}`", r.name));
        }
        if !name_ok(&r.name) {
            problems.push(format!(
                "reminder name `{}` must not be empty or hold / or \\",
                r.name
            ));
        }
        if r.every.tool_calls.is_none() && r.every.prompts.is_none() {
            problems.push(format!("reminder `{}` has no cadence", r.name));
        }
        if !cadence_ok(&r.every) {
            problems.push(format!("reminder `{}` has a zero cadence", r.name));
        }
    }
    for s in &cfg.sections {
        if !names.insert(s.name.clone()) {
            problems.push(format!("duplicate name `{}`", s.name));
        }
        if !name_ok(&s.name) {
            problems.push(format!(
                "section name `{}` must not be empty or hold / or \\",
                s.name
            ));
        }
        if !cadence_ok(&s.refresh) {
            problems.push(format!("section `{}` has a zero cadence", s.name));
        }
        match config::confine_section_path(&cwd.join(".gaff"), &s.file) {
            Err(bad) => problems.push(format!(
                "section `{}`: the path {bad} leaves .gaff/",
                s.name
            )),
            Ok(path) if !path.is_file() => {
                problems.push(format!("section `{}`: .gaff/{} not found", s.name, s.file));
            }
            Ok(path) => {
                // The prefix and separator cost bytes too. A body at or
                // over the cap can never flush without truncation.
                let overhead = format!("[gaff:{}]\n", s.name).len();
                let raw_len = std::fs::metadata(&path).map_or(0, |m| m.len());
                let body_len = usize::try_from(raw_len).unwrap_or(usize::MAX);
                if overhead + body_len > cfg.max_inject_bytes {
                    problems.push(format!(
                        "section `{}`: the body plus its header is {} bytes, over the {}-byte cap",
                        s.name,
                        overhead + body_len,
                        cfg.max_inject_bytes
                    ));
                }
            }
        }
    }

    if problems.is_empty() {
        println!(
            "config ok: {} reminder(s), {} section(s), cap {} bytes",
            cfg.reminders.len(),
            cfg.sections.len(),
            cfg.max_inject_bytes
        );
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            eprintln!("gaff: {p}");
        }
        ExitCode::FAILURE
    }
}

/// `gaff doctor` — report what is live in this clone. This command
/// always exits 0. It reports a problem; it never becomes one.
fn run_doctor() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };

    match config::load(&cwd) {
        Loaded::Ok(cfg) => println!(
            "config:  ok ({} reminder(s), {} section(s))",
            cfg.reminders.len(),
            cfg.sections.len()
        ),
        Loaded::Absent => println!("config:  absent ({})", config::CONFIG_PATH),
        Loaded::Broken(err) => println!("config:  BROKEN — {err}"),
    }

    match resolve_store() {
        Some(store) => {
            println!("state:   {}", store.root_path().display());
            if store.is_degraded() {
                println!("         DEGRADED marker present (a session found a broken config)");
            }
        }
        None => println!("state:   UNRESOLVABLE (set GAFF_STATE_DIR or HOME)"),
    }

    let settings = cwd.join(init::SETTINGS_PATH);
    let registered = std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("gaff hook"));
    if registered {
        println!("hooks:   registered in {}", init::SETTINGS_PATH);
    } else {
        println!("hooks:   NOT registered (run `gaff init`)");
    }
    ExitCode::SUCCESS
}

/// `gaff docs [topic]` — print the bundled documentation.
fn run_docs(args: &[String]) -> ExitCode {
    args.first().map(String::as_str).map_or_else(
        || {
            print!("{}", docs::listing());
            ExitCode::SUCCESS
        },
        |name| {
            docs::topic(name).map_or_else(
                || fail(&format!("unknown topic `{name}`\n{}", docs::listing())),
                |body| {
                    print!("{body}");
                    ExitCode::SUCCESS
                },
            )
        },
    )
}
