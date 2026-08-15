//! The command line surface.
//!
//! The exit-code rule: a gaff *failure* exits 0 or 1, never 2. The
//! agent side treats exit 2 as the blocking code, and no gaff fault
//! may block a session. This covers a config typo, an unwritable state
//! directory, and a bad flag.
//!
//! Two paths exit otherwise, and both are deliberate. A guard refuses
//! a tool call with 2, which is the only channel the harness listens
//! on. `githook` returns the failing command's own code, because a git
//! hook exists to block a commit.
//!
//! This module parses the arguments by hand for that reason, because
//! clap exits 2 on a usage error. It lives in the library rather than
//! in `main.rs`, so the CLI is one testable function and another
//! program can drive gaff without spawning it.
use std::io::{IsTerminal as _, Read as _};
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use crate::config::{self, Loaded};
use crate::engine;
use crate::state::{resolve_root, Store};
use crate::{docs, init};

/// Run gaff with the given arguments, excluding the program name.
///
/// This returns an exit code rather than exiting, so a test can drive
/// the whole CLI surface in process.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("hook") => run_hook(),
        Some("remind") => run_remind(&args[1..]),
        Some("status") => run_status(&args[1..]),
        Some("init") => run_init(&args[1..]),
        Some("check") => run_check(&args[1..]),
        Some("doctor") => run_doctor(),
        Some("profile") => run_profile(&args[1..]),
        Some("trust") => run_trust(),
        Some("githook") => run_githook(&args[1..]),
        Some("log") => run_log(&args[1..]),
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
            "unknown command `{other}` (available: hook, githook, remind, status, init, check, doctor, trust, profile, log, docs)"
        )),
        None => fail("usage: gaff <hook|githook|remind|status|init|check|doctor|trust|profile|log|docs>"),
    }
}

const USAGE: &str = "gaff — a context-lifecycle handler for coding agents

Usage: gaff <command> [options]

Commands:
  hook             Handle one hook event from stdin (the harness calls this)
  remind <text>    Schedule a one-shot reminder N tool calls ahead
  status           Show counters, pending entries, and one-shots
  init [--host H]  Register the agent hooks in the host's settings file
  init --git       Write the git hook scripts declared in the config
  init --github    Generate the workflows declared in the config
  githook <name>   Run one git hook (the installed scripts call this)
  check            Validate .gaff/gaff.yml
                   (--handlers checks the user config;
                    --github reports a workflow that drifted)
  trust            Allow handlers to run in this repo (terminal only)
  doctor           Show what is live in this clone
  profile          Show, list, or set the active profile
  log              Show the injection audit trail for a session
  docs [page]      Print the bundled documentation

Options:
  -h, --help       Print this help
  -V, --version    Print the version

A hook exits 0 or 1, so a gaff fault never blocks a session. Two
things exit otherwise, on purpose: a guard refuses a tool call with 2,
and githook returns the failing command's own code.";

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
    let adapter = crate::adapter::detect(std::env::var("GAFF_HOST").ok().as_deref(), &payload);
    let envelope = (adapter.parse)(payload);

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let loaded = config::load_layered(&cwd);
    let degraded = matches!(loaded, Loaded::Broken(_) | Loaded::Degraded(_));
    let cfg = match loaded {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => {
            eprintln!("gaff: {err}. Continuing without reminders.");
            config::Config::default()
        }
    };

    // A guard is the one thing that may exit 2, and it runs first.
    //
    // It needs only the envelope and the config. Resolving the state
    // directory or the profile before it would make a refusal depend
    // on state gaff might fail to reach, and neither can select a
    // guard anyway.
    if envelope.kind == crate::event::Kind::PreToolCall
        && let Some(refusal) = guard_refusal(&cfg, &envelope)
    {
        eprintln!("{refusal}");
        return ExitCode::from(2);
    }

    let Some(store) = resolve_store() else {
        eprintln!("gaff: no state directory. Set GAFF_STATE_DIR or HOME. Passing through.");
        return ExitCode::SUCCESS;
    };
    if degraded {
        store.mark_degraded();
    }

    // Resolve the profile before the overlay. The session state is
    // authoritative, because it lives outside the repo tree.
    let gaff_dir = cwd.join(".gaff");
    let session = envelope.session_id.clone();
    let profile = config::resolve_profile(
        None,
        std::env::var("GAFF_PROFILE").ok().as_deref(),
        session.as_deref().and_then(|s| store.profile(s)).as_deref(),
        &gaff_dir,
        &cfg,
    );
    let cfg = cfg.with_profile(profile.as_deref());

    let handlers = crate::handler::load().handlers;
    if let Some(context) =
        engine::handle_with(&envelope, &cfg, &store, &gaff_dir, &handlers, Some(&cwd))
    {
        if let Some(sid) = session.as_deref() {
            // Log the normalized name, so the log speaks the same
            // vocabulary a config is written in.
            store.record_injection(sid, envelope.kind.as_str(), &context);
        }
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
    let mut host: Option<String> = None;
    let mut git = false;
    let mut github = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--uninstall" => uninstall = true,
            "--command" => match it.next() {
                Some(v) => command.clone_from(v),
                None => return fail("--command requires a value"),
            },
            "--git" => git = true,
            "--github" => github = true,
            "--host" => match it.next() {
                Some(v) => host = Some(v.clone()),
                None => return fail("--host requires a value"),
            },
            other => return fail(&format!("unexpected argument `{other}`")),
        }
    }
    if git {
        return run_init_git(uninstall);
    }
    if github {
        return run_init_github();
    }
    let adapter = match host.as_deref() {
        None => &crate::adapter::CLAUDE_CODE,
        Some(name) => match crate::adapter::by_name(name) {
            Some(a) => a,
            None => {
                return fail(&format!(
                    "unknown host `{name}` (implemented: {})",
                    crate::adapter::names()
                ));
            }
        },
    };
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let result = if uninstall {
        init::uninstall_for(adapter, &cwd, &command)
    } else {
        init::install_for(adapter, &cwd, &command)
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
fn run_check(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--handlers") {
        return check_handlers();
    }
    if args.iter().any(|a| a == "--github") {
        return check_github();
    }
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let cfg = match config::load_layered(&cwd) {
        Loaded::Absent => {
            println!("no config at {}. Nothing to validate.", config::CONFIG_PATH);
            return ExitCode::SUCCESS;
        }
        Loaded::Broken(err) => return fail(&format!("{}: {err}", config::CONFIG_PATH)),
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
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

    // A guard with a pattern that does not compile blocks nothing, and
    // silence there is the worst outcome: the operator believes a rule
    // is enforced when it is not.
    for guard in &cfg.guards {
        problems.extend(guard.problems());
    }
    problems.extend(cross_reference_problems(&cfg));
    for entry in &cfg.git {
        problems.extend(entry.problems());
    }

    if problems.is_empty() {
        println!(
            "config ok: {} reminder(s), {} section(s), {} guard(s), cap {} bytes",
            cfg.reminders.len(),
            cfg.sections.len(),
            cfg.guards.len(),
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
fn doctor_handlers() {
    let trusted = std::env::current_dir().is_ok_and(|d| crate::handler::is_trusted(&d));
    match crate::handler::load_checked() {
        Err(e) => println!("handlers: {e}"),
        Ok(cfg) if cfg.handlers.is_empty() => println!("handlers: none declared"),
        Ok(cfg) => {
            println!(
                "handlers: {} declared; this repo is {}",
                cfg.handlers.len(),
                if trusted { "trusted" } else { "NOT trusted (run `gaff trust`)" }
            );
            for h in &cfg.handlers {
                let state = if h.problems().is_empty() { "ok " } else { "BAD" };
                println!("  {state} {} -> {}", h.name, h.command.join(" "));
            }
        }
    }
}

fn run_doctor() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };

    let loaded = config::load_layered(&cwd);
    match &loaded {
        Loaded::Degraded(cfg) => println!(
            "config:  DEGRADED — the repo config did not parse; the user config alone is live ({} reminder(s), {} section(s))",
            cfg.reminders.len(),
            cfg.sections.len()
        ),
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

    doctor_hooks(&cwd);
    doctor_guards(&loaded);
    doctor_handlers();
    ExitCode::SUCCESS
}


/// Problems where one part of the config names another part that does
/// not exist, or names the same thing twice.
///
/// Each of these is a rule that never fires, and each was accepted
/// silently before.
fn cross_reference_problems(cfg: &config::Config) -> Vec<String> {
    let mut problems = Vec::new();
    // A name that points at nothing is a rule that never fires. Every
    // one of these was accepted silently before.
    let entry_names: Vec<&String> = cfg
        .reminders
        .iter()
        .map(|r| &r.name)
        .chain(cfg.sections.iter().map(|s| &s.name))
        .collect();
    for (pname, profile) in &cfg.profiles {
        let referenced = profile
            .only
            .iter()
            .flatten()
            .chain(profile.disable.iter())
            .chain(profile.cadence.keys());
        for name in referenced {
            if !entry_names.contains(&name) {
                problems.push(format!(
                    "profile `{pname}`: `{name}` names no reminder or section"
                ));
            }
        }
    }
    if let Some(d) = &cfg.default_profile
        && !cfg.profiles.contains_key(d)
    {
        problems.push(format!("default_profile `{d}` names no profile"));
    }
    for name in &cfg.transitions.clone().unwrap_or_default().agent_may_set {
        if !cfg.profiles.contains_key(name) {
            problems.push(format!(
                "transitions.agent_may_set `{name}` names no profile"
            ));
        }
    }
    for (label, names) in [
        ("guard", cfg.guards.iter().map(|g| &g.name).collect::<Vec<_>>()),
        ("git entry", cfg.git.iter().map(|g| &g.name).collect()),
        ("workflow", cfg.github.iter().map(|w| &w.name).collect()),
    ] {
        for (i, n) in names.iter().enumerate() {
            if names[..i].contains(n) {
                problems.push(format!("duplicate {label} name `{n}`"));
            }
        }
    }
    for wf in &cfg.github {
        problems.extend(wf.problems(&cfg.git));
    }
    problems
}

/// Report where gaff is registered, across every scope the host merges.
///
/// A substring search of one file answered this before, so gaff read as
/// unregistered whenever it was registered at the user scope, and the
/// remedy it printed would have registered it a second time. It also
/// read a file that merely mentioned the command, including one that
/// banned it, as a registration.
fn doctor_hooks(cwd: &std::path::Path) {
    let scopes = [
        (
            "user".to_string(),
            std::env::var("HOME").ok().map(|h| {
                std::path::Path::new(&h)
                    .join(".claude")
                    .join("settings.json")
            }),
        ),
        (
            "repo".to_string(),
            Some(cwd.join(".claude").join("settings.json")),
        ),
        (
            "local".to_string(),
            Some(cwd.join(init::SETTINGS_PATH)),
        ),
    ];
    // The host merges every scope, so gaff must too. Reporting each
    // scope against the full event list called a complete split
    // registration broken.
    let mut union: Vec<String> = Vec::new();
    let mut per_scope: Vec<String> = Vec::new();
    for (label, path) in scopes.into_iter().filter_map(|(l, p)| p.map(|p| (l, p))) {
        let events = registered_events(&path);
        if events.is_empty() {
            continue;
        }
        per_scope.push(format!("{label} ({})", events.len()));
        for e in events {
            if !union.contains(&e) {
                union.push(e);
            }
        }
    }
    let found = !union.is_empty();
    if found {
        let missing: Vec<&str> = crate::adapter::CLAUDE_CODE
            .hook_events
            .iter()
            .copied()
            .filter(|e| !union.iter().any(|got| got == e))
            .collect();
        if missing.is_empty() {
            println!(
                "hooks:   registered ({}), all {} events",
                per_scope.join(" + "),
                union.len()
            );
        } else {
            println!(
                "hooks:   PARTIAL ({}); missing {}",
                per_scope.join(" + "),
                missing.join(", ")
            );
        }
    }
    if !found {
        println!("hooks:   NOT registered (run `gaff init`)");
    }
}

/// Whether a registered command invokes `gaff hook`.
///
/// A tail match got this wrong both ways: it missed a command carrying
/// a trailing flag, which gaff's own installer can produce, and it
/// claimed `mygaff hook`, a different binary.
fn is_gaff_hook_command(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    while let Some(word) = parts.next() {
        let stem = std::path::Path::new(word)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());
        if stem.as_deref() == Some("gaff") {
            return parts.next() == Some("hook");
        }
    }
    false
}

/// The events a settings file registers `gaff hook` on.
///
/// This walks the structure rather than searching the text, so a file
/// that only mentions the command does not read as a registration.
fn registered_events(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(hooks) = json.get("hooks").and_then(|h| h.as_object()) else {
        return Vec::new();
    };
    hooks
        .iter()
        .filter(|(_, groups)| {
            groups.as_array().is_some_and(|gs| {
                gs.iter().any(|g| {
                    g.get("hooks").and_then(|h| h.as_array()).is_some_and(|hs| {
                        hs.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .is_some_and(is_gaff_hook_command)
                        })
                    })
                })
            })
        })
        .map(|(event, _)| event.clone())
        .collect()
}

/// Report whether the guards are live.
///
/// A guard that silently stops working is worse than no guard, and the
/// ways it can stop are quiet: an unreadable user config, a typo in it,
/// or a pattern that does not compile. This is the command that answers
/// "is my refusal actually armed".
fn doctor_guards(loaded: &Loaded) {
    let (Loaded::Ok(cfg) | Loaded::Degraded(cfg)) = loaded else {
        println!("guards:  NONE ACTIVE — the config did not load");
        println!("         Every refusal is off until the config parses.");
        return;
    };
    if cfg.guards.is_empty() {
        println!("guards:  none active");
        println!("         (a broken or unreadable user config disarms every guard)");
        return;
    }
    println!("guards:  {} active", cfg.guards.len());
    for g in &cfg.guards {
        let problems = g.problems();
        let state = if problems.is_empty() { "ok " } else { "BAD" };
        println!("  {state} {} on {} ({})", g.name, g.tool, g.field);
        for p in problems {
            println!("      {p}");
        }
    }
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

/// `gaff profile [show|list|set <name>] [--session <sid>]`
///
/// Profiles are advisory. gaff blocks nothing, and an agent that can
/// write files can edit the config regardless. The transition policy
/// refuses the agent-facing path, so an unsanctioned switch is at least
/// not a supported one. Structural identity decides who is asking: a
/// terminal on stdin is a human, anything else is an agent.
fn run_profile(args: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = match config::load_layered(&cwd) {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => {
            eprintln!("gaff: the config is not valid: {err}");
            return ExitCode::FAILURE;
        }
    };
    let gaff_dir = cwd.join(".gaff");

    let mut it = args.iter().map(String::as_str);
    let sub = it.next();
    let mut positional: Option<String> = None;
    let mut session_flag: Option<String> = None;
    let mut rest: Vec<&str> = it.collect();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--session" => match rest.get(i + 1) {
                Some(v) => {
                    session_flag = Some((*v).to_string());
                    i += 2;
                }
                None => return fail("--session requires a value"),
            },
            other if positional.is_none() && !other.starts_with("--") => {
                positional = Some(other.to_string());
                i += 1;
            }
            other => return fail(&format!("unknown option `{other}`")),
        }
    }
    rest.clear();

    let session = session_from(session_flag.as_deref());

    match sub {
        None | Some("show") => {
            let Some(store) = resolve_store() else {
                return fail("no state directory");
            };
            let from_session = session.as_deref().and_then(|s| store.profile(s));
            let active = config::resolve_profile(
                None,
                std::env::var("GAFF_PROFILE").ok().as_deref(),
                from_session.as_deref(),
                &gaff_dir,
                &cfg,
            );
            match active {
                Some(name) => println!("{name}"),
                None => println!("(none)"),
            }
            ExitCode::SUCCESS
        }
        Some("list") => {
            if cfg.profiles.is_empty() {
                println!("(no profiles declared in .gaff/gaff.yml)");
            }
            for name in cfg.profiles.keys() {
                let who = if cfg.transitions.clone().unwrap_or_default().agent_may_set(name) {
                    "agent or human"
                } else {
                    "human only"
                };
                println!("{name}\t{who}");
            }
            ExitCode::SUCCESS
        }
        Some("set") => set_profile(&cfg, positional, session),
        Some(other) => fail(&format!(
            "unknown profile command `{other}` (available: show, list, set)"
        )),
    }
}

/// `gaff log [--session <sid>]`
///
/// Print the injection audit trail: what gaff put into the session, in
/// order, with the byte count and the entry names.
fn run_log(args: &[String]) -> ExitCode {
    let mut session_flag: Option<String> = None;
    let mut it = args.iter().map(String::as_str);
    while let Some(arg) = it.next() {
        match arg {
            "--session" => match it.next() {
                Some(v) => session_flag = Some(v.to_string()),
                None => return fail("--session requires a value"),
            },
            other => return fail(&format!("unknown option `{other}`")),
        }
    }
    let Some(store) = resolve_store() else {
        return fail("no state directory");
    };
    let Some(session) = session_from(session_flag.as_deref()) else {
        let sessions = store.sessions();
        if sessions.is_empty() {
            return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID");
        }
        eprintln!("gaff: no session given. Known sessions:");
        for s in sessions {
            eprintln!("  {s}");
        }
        return ExitCode::FAILURE;
    };
    let lines = store.injections(&session);
    if lines.is_empty() {
        println!("no injections recorded for session {session}");
        return ExitCode::SUCCESS;
    }
    println!("{:<20} {:>7}  ENTRIES", "EVENT", "BYTES");
    for line in lines {
        let event = line["event"].as_str().unwrap_or("?");
        let bytes = line["bytes"].as_u64().unwrap_or(0);
        let entries: Vec<&str> = line["entries"]
            .as_array()
            .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        println!("{event:<20} {bytes:>7}  {}", entries.join(", "));
    }
    ExitCode::SUCCESS
}

/// Apply `gaff profile set`. Structural identity decides who is asking:
/// a terminal on stdin is a human, anything else is an agent.
fn set_profile(
    cfg: &config::Config,
    name: Option<String>,
    session: Option<String>,
) -> ExitCode {
    let Some(name) = name else {
        return fail("usage: gaff profile set <name> [--session <sid>]");
    };
    if !cfg.profiles.contains_key(&name) {
        return fail(&format!("unknown profile `{name}`"));
    }
    let Some(session) = session else {
        return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID");
    };
    if !std::io::stdin().is_terminal() && !cfg.transitions.clone().unwrap_or_default().agent_may_set(&name) {
        return fail(&format!(
            "profile `{name}` is human-only. Add it to transitions.agent_may_set to allow an agent switch."
        ));
    }
    let Some(store) = resolve_store() else {
        return fail("no state directory");
    };
    match store.set_profile(&session, &name) {
        Ok(true) => {
            println!("profile set to `{name}`; the next flush re-primes the sections");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("profile is already `{name}`");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("cannot write the profile: {e}")),
    }
}

/// `gaff trust`
///
/// Record consent for this repo to run handlers. A handler's child runs
/// with the repo as its working directory, and many ordinary tools read
/// executable settings from there, so consent is per-repo and explicit.
/// Only a human at a terminal may grant it; the agent path is refused
/// by the same structural identity rule `gaff profile set` uses.
fn run_trust() -> ExitCode {
    if !std::io::stdin().is_terminal() {
        return fail(
            "gaff trust must be run from a terminal. An agent may not grant itself the right to run commands.",
        );
    }
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    match crate::handler::trust(&cwd) {
        Ok(true) => {
            println!("this repo may now run handlers");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("this repo was already trusted");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("cannot record the consent: {e}")),
    }
}

/// Validate the user-scoped handler config.
///
/// This is separate from `gaff check`, which stays repo-only so it
/// behaves the same in CI as it does locally.
fn check_handlers() -> ExitCode {
    let cfg = match crate::handler::load_checked() {
        Ok(cfg) => cfg,
        Err(e) => return fail(&e),
    };
    if cfg.handlers.is_empty() {
        println!("no handlers declared");
        return ExitCode::SUCCESS;
    }
    let mut bad = false;
    for h in &cfg.handlers {
        let problems = h.problems();
        if problems.is_empty() {
            println!("ok   {} -> {}", h.name, h.command.join(" "));
        } else {
            bad = true;
            for p in problems {
                println!("FAIL {p}");
            }
        }
    }
    let trusted = std::env::current_dir()
        .is_ok_and(|d| crate::handler::is_trusted(&d));
    if !trusted {
        println!("note: this repo is not trusted, so no handler runs here. Run `gaff trust`.");
    }
    if bad { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// `gaff init --git` — write the git hook scripts the config declares.
///
/// Running this is the consent step, the same deliberate act as
/// `pre-commit install`. gaff writes one script per declared hook and
/// keeps any hook it did not write.
fn run_init_git(uninstall: bool) -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    if uninstall {
        return match crate::githook::uninstall(&cwd) {
            Ok(removed) if removed.is_empty() => {
                println!("no gaff git hooks were installed");
                ExitCode::SUCCESS
            }
            Ok(removed) => {
                println!("removed: {}", removed.join(", "));
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("cannot remove the hooks: {e}")),
        };
    }
    let cfg = match config::load_layered(&cwd) {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => return fail(&err),
    };
    let mut bad = false;
    for entry in &cfg.git {
        for problem in entry.problems() {
            eprintln!("gaff: {problem}");
            bad = true;
        }
    }
    if bad {
        return ExitCode::FAILURE;
    }
    if cfg.git.is_empty() {
        println!("the config declares no git entries. Nothing to install.");
        return ExitCode::SUCCESS;
    }
    match crate::githook::install(&cwd, &cfg.git, "gaff githook") {
        Ok(written) => {
            println!("installed: {}", written.join(", "));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("cannot write the hooks: {e}")),
    }
}

/// `gaff init --github` — generate the declared workflows.
fn run_init_github() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let cfg = match config::load_layered(&cwd) {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => return fail(&err),
    };
    if let Some(code) = report_github_problems(&cfg) {
        return code;
    }
    if cfg.github.is_empty() {
        println!("the config declares no workflows. Nothing to generate.");
        return ExitCode::SUCCESS;
    }
    match crate::ghworkflow::write_all(&cwd, &cfg.github, &cfg.git) {
        Ok(paths) => {
            println!("generated: {}", paths.join(", "));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("cannot write the workflows: {e}")),
    }
}

/// Print every workflow problem. Returns a failure code when any
/// workflow is unusable.
fn report_github_problems(cfg: &config::Config) -> Option<ExitCode> {
    let mut bad = false;
    for wf in &cfg.github {
        for problem in wf.problems(&cfg.git) {
            eprintln!("gaff: {problem}");
            bad = true;
        }
    }
    bad.then_some(ExitCode::FAILURE)
}

/// `gaff check --github` — report a workflow that drifted from the
/// config.
///
/// A generated file that someone edited by hand is the failure this
/// catches. It exits 1, so CI can run it.
fn check_github() -> ExitCode {
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let cfg = match config::load_layered(&cwd) {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => return fail(&err),
    };
    if let Some(code) = report_github_problems(&cfg) {
        return code;
    }
    if cfg.github.is_empty() {
        println!("no workflows declared");
        return ExitCode::SUCCESS;
    }
    let mut drifted = false;
    for wf in &cfg.github {
        match crate::ghworkflow::drift(wf, &cfg.git, &cwd) {
            crate::ghworkflow::Drift::Match => println!("ok      {}", wf.name),
            crate::ghworkflow::Drift::Missing => {
                println!("MISSING {} (run `gaff init --github`)", wf.name);
                drifted = true;
            }
            crate::ghworkflow::Drift::Differs => {
                println!("DRIFTED {} (run `gaff init --github`)", wf.name);
                drifted = true;
            }
        }
    }
    for orphan in crate::ghworkflow::orphans(&cwd, &cfg.github) {
        println!("ORPHAN  {orphan} (generated by gaff, declared by nothing)");
        drifted = true;
    }
    if drifted { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// `gaff githook <name> [args...]` — run one git hook.
///
/// The installed scripts call this. Unlike `gaff hook`, this returns
/// the failing command's exit code, because a git hook exists to block
/// the commit or the push.
fn run_githook(args: &[String]) -> ExitCode {
    let Some(hook) = args.first() else {
        return fail("usage: gaff githook <hook-name> [args...]");
    };
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let cfg = match config::load_layered(&cwd) {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
        Loaded::Absent => config::Config::default(),
        Loaded::Broken(err) => {
            // A broken config must not silently pass a check that was
            // meant to run.
            return fail(&format!("{err}. Refusing to skip the {hook} checks."));
        }
    };
    let code = crate::githook::run(&cwd, &cfg.git, hook, &args[1..]);
    u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from)
}

/// The message of the first guard that refuses this call.
///
/// This is the only path in gaff that leads to exit 2. Every failure
/// path, including a broken guard, still degrades, because a gaff
/// fault must never block a session.
fn guard_refusal(cfg: &config::Config, envelope: &crate::event::Envelope) -> Option<String> {
    let tool = envelope.tool_name.as_deref()?;
    let value = |field: &str| {
        envelope
            .raw
            .get("tool_input")
            .and_then(|i| i.get(field))
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
    };
    let hit = crate::guard::first_refusal(&cfg.guards, tool, &value)?;
    Some(format!(
        "gaff: refused by the guard `{}`.\n\n{}",
        hit.name, hit.message
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// The exit-code rule is the load-bearing invariant: exit 2 is the
    /// agent side's blocking code, and no gaff failure may block a
    /// session. Every bad-input path must exit 1, never 2.
    #[test]
    fn no_usage_error_ever_exits_two() {
        let bad = [
            vec!["nonsense"],
            vec!["remind"],
            vec!["remind", "text", "--after"],
            vec!["remind", "text", "--after", "notanumber"],
            vec!["remind", "--id"],
            vec!["init", "--command"],
            vec!["init", "--host"],
            vec!["init", "--host", "nosuchhost"],
            vec!["profile", "nonsense"],
            vec!["profile", "set"],
            vec!["log", "--session"],
            vec!["log", "--bogus"],
        ];
        for case in bad {
            let code = run(&args(&case));
            assert_eq!(
                format!("{code:?}"),
                format!("{:?}", ExitCode::FAILURE),
                "`gaff {}` must exit 1, never 2",
                case.join(" ")
            );
        }
    }

    #[test]
    fn help_and_version_succeed() {
        for case in [vec!["--help"], vec!["-h"], vec!["help"], vec!["--version"], vec!["-V"], vec!["version"]] {
            let code = run(&args(&case));
            assert_eq!(
                format!("{code:?}"),
                format!("{:?}", ExitCode::SUCCESS),
                "`gaff {}` must succeed",
                case.join(" ")
            );
        }
    }

    #[test]
    fn the_usage_text_lists_every_command_the_dispatcher_accepts() {
        // A command that dispatches but is undocumented is a trap.
        for cmd in [
            "hook", "remind", "status", "init", "check", "doctor", "trust", "profile", "log",
            "docs",
        ] {
            assert!(USAGE.contains(cmd), "the usage text omits `{cmd}`");
        }
    }

    #[test]
    fn no_arguments_is_a_usage_error_not_a_panic() {
        let code = run(&[]);
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
