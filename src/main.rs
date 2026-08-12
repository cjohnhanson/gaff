//! gaff — context-lifecycle handler for coding agents.
//!
//! Exit-code invariant: every gaff invocation exits 0 or 1, never 2.
//! Exit 2 is the blocking code on the agent side, and no gaff failure —
//! config typo, unwritable state, bad flags — may ever block a session.
//! Argument parsing is by hand for exactly this reason: clap exits 2 on
//! usage errors.

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
        Some(other) => fail(&format!(
            "unknown command `{other}` (available: hook, remind, status, init, check, doctor, docs)"
        )),
        None => fail("usage: gaff <hook|remind|status|init|check|doctor|docs>"),
    }
}

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

/// Read one hook payload from stdin, emit a response on stdout.
/// Failure semantics: unreadable/unparseable stdin exits 1 (Claude Code
/// treats 1 as a non-blocking error); everything downstream degrades to
/// silent passthrough at exit 0.
fn run_hook() -> ExitCode {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return fail("could not read stdin");
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&input) else {
        return fail("invalid JSON on stdin");
    };
    let envelope = Envelope::from_claude_code(payload);

    let Some(store) = resolve_store() else {
        eprintln!("gaff: no state dir (set GAFF_STATE_DIR or HOME); passing through");
        return ExitCode::SUCCESS;
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = match config::load(&cwd) {
        Loaded::Ok(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => {
            eprintln!(
                "gaff: config error in {}: {err}; continuing without reminders",
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
/// Schedules a one-shot N tool calls into the session's future.
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
        return fail("no session: pass --session or set CLAUDE_CODE_SESSION_ID");
    };
    let Some(store) = resolve_store() else {
        return fail("no state dir (set GAFF_STATE_DIR or HOME)");
    };

    let counts = store.counts(&session);
    let id = id.unwrap_or_else(|| format!("r{}", store.oneshots(&session).len() + 1));
    let at = counts.tool_calls + after;
    match store.write_oneshot(&session, &id, after, at, &text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("could not write reminder: {e}")),
    }
}

/// `gaff status [--session <sid>]` — read-side counters as JSON.
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
        return fail("no session: pass --session or set CLAUDE_CODE_SESSION_ID");
    };
    let Some(store) = resolve_store() else {
        return fail("no state dir (set GAFF_STATE_DIR or HOME)");
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

/// `gaff init [--uninstall] [--command <cmd>]` — register or remove the
/// hook entries in `.claude/settings.local.json`.
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
        return fail("cannot resolve working directory");
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

/// `gaff check` — validate `.gaff/gaff.yml`. Exit 1 on an invalid
/// config (this is the one place loud failure is the job).
fn run_check() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve working directory");
    };
    let cfg = match config::load(&cwd) {
        Loaded::Absent => {
            println!("no config ({}); nothing to validate", config::CONFIG_PATH);
            return ExitCode::SUCCESS;
        }
        Loaded::Broken(err) => return fail(&format!("{}: {err}", config::CONFIG_PATH)),
        Loaded::Ok(cfg) => cfg,
    };

    let mut problems = Vec::new();
    let mut names = std::collections::HashSet::new();
    let cadence_ok =
        |e: &config::Every| e.tool_calls.is_none_or(|n| n > 0) && e.prompts.is_none_or(|n| n > 0);
    for r in &cfg.reminders {
        if !names.insert(r.name.clone()) {
            problems.push(format!("duplicate name `{}`", r.name));
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
        if !cadence_ok(&s.refresh) {
            problems.push(format!("section `{}` has a zero cadence", s.name));
        }
        if !cwd.join(".gaff").join(&s.file).is_file() {
            problems.push(format!("section `{}`: .gaff/{} not found", s.name, s.file));
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

/// `gaff doctor` — report what is live in this clone. Always exits 0;
/// it reports problems, it is not one.
fn run_doctor() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve working directory");
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
                println!("         DEGRADED marker present (a session hit a broken config)");
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

/// `gaff docs [topic]` — bundled documentation.
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
