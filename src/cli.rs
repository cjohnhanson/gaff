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
//! This module parses arguments by hand for that reason, because
//! clap exits 2 on a usage error. It lives in the library rather than
//! in `main.rs`, so the CLI is one testable function and another
//! program can drive gaff without spawning it.
use std::io::{IsTerminal as _, Read as _};
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use crate::config::{self, Loaded};
use crate::engine;
use crate::state::{Store, resolve_root};
use crate::{docs, init};

/// Run gaff with the given arguments, excluding the program name.
///
/// This returns an exit code rather than exiting, so a test can drive
/// the whole CLI surface in process.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("hook") => run_hook(&args[1..]),
        Some("remind") => run_remind(&args[1..]),
        Some("status") => run_status(&args[1..]),
        Some("init") => run_init(&args[1..]),
        Some("check") => run_check(&args[1..]),
        Some("doctor") => run_doctor(),
        Some("profile") => run_profile(&args[1..]),
        Some("trust") => run_trust(),
        Some("allow") => run_allow(&args[1..]),
        Some("githook") => run_githook(&args[1..]),
        Some("log") => run_log(&args[1..]),
        Some("docs") => run_docs(&args[1..]),
        Some("prime") => {
            print!("{}", prime());
            ExitCode::SUCCESS
        }
        Some("--version" | "-V" | "version") => {
            println!("gaff {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => fail(&format!(
            "unknown command `{other}` (available: {})",
            COMMANDS.join(", ")
        )),
        None => fail(&format!("usage: gaff <{}>", COMMANDS.join("|"))),
    }
}

/// The one-line description. The usage text opens with it and `prime`
/// prints it, and a test holds the two byte-equal.
pub const ABOUT: &str = "a context-lifecycle handler for coding agents";

const USAGE: &str = "gaff — a context-lifecycle handler for coding agents

Usage: gaff <command> [options]

Commands:
  hook             Handle one hook event from stdin (the harness calls this)
  remind <text>    Schedule a one-shot reminder N tool calls ahead
                   (--at stop holds the stop; --times N lets go after N)
  status           Show counters, pending entries, and one-shots
  init [--host H]  Register the agent hooks in the host's settings file
  init --git       Write the git hook scripts declared in the config
  init --github    Generate the workflows declared in the config
  githook <name>   Run one git hook (the installed scripts call this)
  check            Validate .gaff/gaff.yml
                   (--handlers checks the user config;
                    --github reports a workflow that drifted)
  trust            Allow handlers to run in this repo (the hook refuses
                   it from an agent; run it from your shell)
  allow <guard>    Let the next call that guard would refuse through,
                   once (the hook refuses it from an agent)
  doctor           Show what is live in this clone
  profile          Show, list, or set the active profile
  log              Show the injection audit trail for a session
  docs [page]      Print the bundled documentation
  prime            Print what gaff is and how to use it, for an agent's context

Options:
  -h, --help       Print this help
  -V, --version    Print the version

A hook exits 0 or 1, so a gaff fault never blocks a session. Three
things exit otherwise, on purpose: a guard refuses a tool call with 2,
a goal refuses a stop with 2, and githook returns the failing
command's own code.";

/// Every command `run` dispatches. The usage test and the prime test
/// both walk it, so a command that dispatches but is undocumented, or
/// documented but does not dispatch, fails a test.
const COMMANDS: [&str; 13] = [
    "hook", "githook", "remind", "allow", "status", "init", "check", "doctor", "trust", "profile",
    "log", "docs", "prime",
];

/// The prime: what gaff is, for an agent's context.
///
/// A pure function of the binary. It states what gaff does from the
/// host's hooks, how an injected entry and a refusal look, and where
/// config lives, then the commands an agent reaches for. It names no
/// host, no sibling tool, and no harness syntax, and it directs
/// nothing: which reminders to set, and when, is the caller's policy.
/// Under 700 bytes, checked by a test.
#[must_use]
pub fn prime() -> String {
    format!(
        "# gaff\n\
         {ABOUT}\n\
         From the host's hooks, gaff runs guards on tool calls, injects sections and \
         reminders at session start and on a cadence, and can hold the stop until a \
         reminder clears. Each injected entry opens with a tag, gaff:<name> in square \
         brackets, on its own line. A refused tool call names its guard. Repo config is \
         .gaff/gaff.yml; user config is $HOME/.config/gaff/gaff.yml.\n\
         Commands:\n\
         \x20 gaff doctor\n\
         \x20 gaff status\n\
         \x20 gaff remind <text> (--after <n> | --at stop) [--id <id>]\n\
         \x20 gaff remind --clear --id <id>\n\
         \x20 gaff check\n\
         More: gaff --help; gaff docs\n"
    )
}

/// How many stops in a row a goal may refuse before gaff gives up on
/// it. A condition that can never be met would otherwise end the
/// session's ability to end.
const MAX_STOP_REFUSALS: u32 = 12;

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
fn run_hook(args: &[String]) -> ExitCode {
    // Every other subcommand rejects a flag it does not know. `hook`
    // took its whole input from stdin and ignored the rest, so a typo
    // read as a working invocation.
    if let Some(unexpected) = args.first() {
        return fail(&format!(
            "unexpected argument `{unexpected}`. `gaff hook` reads its payload from stdin."
        ));
    }
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
            // Guards live only in the user config, so a schema error
            // anywhere in that file — in a section with nothing to do
            // with guards — turns off every refusal. Saying "continuing
            // without reminders" named the wrong subsystem and read
            // like a cosmetic degradation.
            eprintln!(
                "gaff: {err}. Continuing without reminders, and any guard declared there is NOT active."
            );
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
        && let Some((guard, refusal)) = guard_refusal(&cfg, &envelope, adapter)
    {
        // A one-shot allowance from the human lets this call through.
        // It needs the store, so it is the one guard-path step that
        // resolves state; a missing store means no allowance, which is
        // the safe reading.
        let allowed = envelope
            .session_id
            .as_deref()
            .zip(resolve_store())
            .is_some_and(|(sid, store)| store.take_allowance(sid, &guard));
        if allowed {
            eprintln!(
                "gaff: the guard `{guard}` would have refused this call. Allowed once by the user."
            );
        } else {
            eprintln!("{refusal}");
            return ExitCode::from(2);
        }
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

    // Stop is the last moment before the model walks away, so it is the
    // one flush point that is a decision rather than a moment, and the
    // one that can still be refused.
    if envelope.kind == crate::event::Kind::Stop
        && let Some(sid) = session.as_deref()
        && let Some(code) = refuse_stop(&handlers, &store, sid, &cwd)
    {
        return code;
    }

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

/// Whether anything refuses this stop, and the code to exit with.
///
/// Two things can: a handler the user configured with `blocks: true`,
/// whose command decides, and a hold the session set for itself, which
/// is text the model judges. The first runs a command and lives in the
/// user config, which is why it may run at all. The second runs nothing,
/// which is why an agent may set one — `gaff trust` exists so an agent
/// cannot schedule command execution for itself.
fn refuse_stop(
    handlers: &[crate::handler::Handler],
    store: &Store,
    session: &str,
    cwd: &std::path::Path,
) -> Option<ExitCode> {
    let blocking: Vec<crate::handler::Handler> =
        handlers.iter().filter(|h| h.blocks).cloned().collect();
    if !blocking.is_empty() {
        let armed = |_: &str| true;
        let outputs = crate::handler::run_due(
            &blocking,
            crate::event::Kind::Stop.as_str(),
            session,
            cwd,
            &armed,
        );
        if let Some(failed) = outputs.iter().find(|o| o.failed) {
            let streak = store.record_stop_refusal(session);
            if streak <= MAX_STOP_REFUSALS {
                eprintln!(
                    "gaff: the handler `{}` failed, so this is not done.",
                    failed.name
                );
                if let Some(text) = &failed.text {
                    eprintln!("\n{text}");
                }
                return Some(ExitCode::from(2));
            }
            eprintln!(
                "gaff: the handler `{}` has refused {streak} stops in a row, so gaff is letting this one through.",
                failed.name
            );
        }
    }

    if let Some(hold) = store.holds(session).first() {
        let id = &hold.id;
        let streak = store.record_stop_refusal(session);
        if streak <= MAX_STOP_REFUSALS {
            // A hold with a budget lets go on its own once it is spent.
            // That is how a session says "push back this many times,
            // then let me stop" rather than "hold until cleared".
            let spent = store.refuse_hold(session, id);
            if spent {
                eprintln!(
                    "gaff: the hold `{id}` has pushed back {} time(s), which is what it asked for. Letting the stop through.",
                    hold.refused
                );
                store.clear_hold(session, Some(id));
                store.clear_stop_refusals(session);
                return None;
            }
            let remaining = hold
                .times
                .map(|t| {
                    let left = t.saturating_sub(hold.refused + 1);
                    if left == 0 {
                        " (the next stop goes through)".to_string()
                    } else {
                        format!(" ({left} more, then it lets go)")
                    }
                })
                .unwrap_or_default();
            eprintln!(
                "gaff: held by `{id}`{remaining}. Clear it with `gaff remind --clear --id {id}` once it is true.\n\n{}",
                hold.text
            );
            return Some(ExitCode::from(2));
        }
        // A hold nothing clears would end the session's ability to end,
        // and nothing inside the session could undo it.
        eprintln!(
            "gaff: the hold `{id}` has refused {streak} stops in a row, so gaff is letting this one through."
        );
        store.clear_hold(session, None);
    }
    store.clear_stop_refusals(session);
    None
}

/// The parsed arguments of `gaff remind`. Parsing is a pure function
/// so a test can check what the command accepts without a store.
#[derive(Debug, Default, PartialEq, Eq)]
struct RemindArgs {
    text: Option<String>,
    after: Option<u64>,
    id: Option<String>,
    session: Option<String>,
    at_stop: bool,
    clear: bool,
    times: Option<u32>,
}

/// Parse `gaff remind` arguments. `Err` carries the usage message.
fn parse_remind(args: &[String]) -> Result<RemindArgs, String> {
    let mut r = RemindArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--after" => match it.next().map(|v| v.parse::<u64>()) {
                Some(Ok(n)) => r.after = Some(n),
                _ => return Err("--after requires a non-negative integer".into()),
            },
            // A one-shot normally fires at a tool-call count. `--at
            // stop` fires it at the stop instead, and holds the stop
            // open until it is cleared.
            "--at" => match it.next().map(String::as_str) {
                Some("stop") => r.at_stop = true,
                Some(other) => return Err(format!("--at takes `stop`, not `{other}`")),
                None => return Err("--at requires a value".into()),
            },
            "--clear" => r.clear = true,
            // How many stops the hold refuses before it lets go on its
            // own. Without it, the hold lasts until cleared.
            "--times" => match it.next().map(|v| v.parse::<u32>()) {
                Some(Ok(n)) if n > 0 => r.times = Some(n),
                _ => return Err("--times requires a positive integer".into()),
            },
            "--id" => match it.next() {
                Some(v) => r.id = Some(v.clone()),
                None => return Err("--id requires a value".into()),
            },
            "--session" => match it.next() {
                Some(v) => r.session = Some(v.clone()),
                None => return Err("--session requires a value".into()),
            },
            other if r.text.is_none() && !other.starts_with("--") => {
                r.text = Some(other.to_string());
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    if r.times.is_some() && !r.at_stop {
        return Err("--times applies to --at stop".into());
    }
    Ok(r)
}

/// `gaff remind <text> --after <N> [--id <id>] [--session <sid>]`
///
/// Schedule a one-shot reminder N tool calls into the session's future.
fn run_remind(args: &[String]) -> ExitCode {
    let RemindArgs {
        text,
        after,
        id,
        session: session_flag,
        at_stop,
        clear,
        times,
    } = match parse_remind(args) {
        Ok(r) => r,
        Err(msg) => return fail(&msg),
    };

    let Some(session) = session_from(session_flag.as_deref()) else {
        return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID.");
    };
    let Some(store) = resolve_store() else {
        return fail("no state directory. Set GAFF_STATE_DIR or HOME.");
    };

    if clear {
        store.clear_hold(&session, id.as_deref());
        println!("released");
        return ExitCode::SUCCESS;
    }

    let Some(text) = text else {
        return fail(
            "usage: gaff remind <text> (--after <N> | --at stop) [--id <id>] [--session <sid>]",
        );
    };

    if at_stop {
        let id = id.unwrap_or_else(|| "hold".to_string());
        return match store.write_hold(&session, &id, &text, times) {
            Ok(()) => {
                match times {
                    Some(n) => println!(
                        "holding the stop as `{id}` for {n} refusal(s). Release it early with `gaff remind --clear --id {id}`."
                    ),
                    None => println!(
                        "holding the stop as `{id}`. Release it with `gaff remind --clear --id {id}`."
                    ),
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("cannot write the hold: {e}")),
        };
    }

    let Some(after) = after else {
        return fail("--after is required, or use --at stop");
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
        if command != "gaff hook" {
            return fail(
                "--command applies to the agent hooks, not to --git. A git hook always calls this binary by its own absolute path.",
            );
        }
        if host.is_some() {
            return fail("--host applies to the agent hooks, not to --git");
        }
        return run_init_git(uninstall);
    }
    if github {
        if command != "gaff hook" {
            return fail(
                "--command applies to the agent hooks, not to --github. A workflow step runs what the config declares.",
            );
        }
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
/// Validate a config's reminders and sections.
///
/// Split out of `run_check` so each half stays readable. Every problem
/// here is a rule that reads as configured but can never fire.
fn entry_problems(cfg: &config::Config, cwd: &std::path::Path) -> Vec<String> {
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
        // An empty reminder injects a bare `[gaff:name]` label with
        // nothing under it, which costs bytes and says nothing.
        if r.text.trim().is_empty() {
            problems.push(format!("reminder `{}` has no text", r.name));
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
        // Read it the way the hook path reads it, so check sees a
        // symlinked or oversized body rather than blessing it.
        if let Err(msg) = config::read_section_body(s, &cwd.join(".gaff")) {
            problems.push(msg);
        }
        match config::section_path(s, &cwd.join(".gaff")) {
            Err(msg) => problems.push(msg),
            Ok(path) if !path.is_file() => {
                problems.push(format!(
                    "section `{}`: {} not found",
                    s.name,
                    path.display()
                ));
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
    problems
}

fn run_check(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--handlers") {
        return check_handlers();
    }
    if args.iter().any(|a| a == "--github") {
        return check_github();
    }
    // Every other subcommand rejects a flag it does not know. A
    // silently ignored `--handler` would report the repo config as ok
    // and read as though the handlers had passed.
    if let Some(unknown) = args.iter().find(|a| a.starts_with('-')) {
        return fail(&format!(
            "unexpected argument `{unknown}` (check takes --handlers or --github)"
        ));
    }
    let Ok(cwd) = std::env::current_dir() else {
        return fail("cannot resolve the working directory");
    };
    let cfg = match config::load_layered(&cwd) {
        Loaded::Absent => {
            let stale = crate::githook::stale_installs(&cwd, &[]);
            if stale.is_empty() {
                println!("no config at {}. Nothing to validate.", config::CONFIG_PATH);
                return ExitCode::SUCCESS;
            }
            // A gaff hook script is installed and there is no config to
            // drive it, so every commit in this repo is already being
            // refused. Reporting "nothing to validate" called that
            // clean.
            eprintln!(
                "gaff: {} is missing or empty, and gaff's {} hook is installed, so it refuses every run. Restore the config, or remove the hook with `gaff init --git --uninstall`.",
                config::CONFIG_PATH,
                stale.join(", ")
            );
            return ExitCode::FAILURE;
        }
        // The error already names the file it came from, and that file
        // may be the user's config rather than the repo's. Prefixing
        // the repo path sent readers to the wrong file.
        Loaded::Broken(err) => return fail(&err),
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => cfg,
    };

    let mut problems = entry_problems(&cfg, &cwd);

    // Git hooks never travel with a clone, so a fresh checkout of a
    // repo that declares them has none installed. Nothing said so, and
    // every commit skipped every check while check passed.
    // `check` already reports a git hook that is declared but not
    // installed, which sets the expectation that it covers install
    // drift generally. A CI job running plain `check` was missing a
    // hand-edited or orphaned workflow.
    for wf in &cfg.github {
        match crate::ghworkflow::drift(wf, &cfg.git, &cwd) {
            crate::ghworkflow::Drift::Match => {}
            crate::ghworkflow::Drift::Missing => problems.push(format!(
                "the workflow `{}` is declared but not generated. Run `gaff init --github`.",
                wf.name
            )),
            crate::ghworkflow::Drift::Differs => problems.push(format!(
                "the workflow `{}` differs from the config. Run `gaff init --github`.",
                wf.name
            )),
        }
    }
    for orphan in crate::ghworkflow::orphans(&cwd, &cfg.github) {
        problems.push(format!(
            "{orphan} was generated by gaff and the config declares nothing for it. Remove it, or declare it."
        ));
    }
    let missing = crate::githook::missing_installs(&cwd, &cfg.git);
    if !missing.is_empty() {
        let (subject, verb) = if missing.len() == 1 {
            ("hook", "is")
        } else {
            ("hooks", "are")
        };
        problems.push(format!(
            "the config declares the {subject} {}, which {verb} not installed, so nothing runs on {}. Run `gaff init --git`.",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" }
        ));
    }

    let mut warnings = Vec::new();
    for guard in &cfg.guards {
        problems.extend(guard.problems());
        warnings.extend(guard.warnings());
    }
    problems.extend(cross_reference_problems(&cfg));
    for entry in &cfg.git {
        problems.extend(entry.problems());
    }

    for w in &warnings {
        eprintln!("gaff: {w}");
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
                if trusted {
                    "trusted"
                } else {
                    "NOT trusted (run `gaff trust`)"
                }
            );
            for h in &cfg.handlers {
                let state = if h.problems().is_empty() {
                    "ok "
                } else {
                    "BAD"
                };
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
        (
            "guard",
            cfg.guards.iter().map(|g| &g.name).collect::<Vec<_>>(),
        ),
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
        ("local".to_string(), Some(cwd.join(init::SETTINGS_PATH))),
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

/// Report whether guards are live.
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
                let who = if cfg
                    .transitions
                    .clone()
                    .unwrap_or_default()
                    .agent_may_set(name)
                {
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
fn set_profile(cfg: &config::Config, name: Option<String>, session: Option<String>) -> ExitCode {
    let Some(name) = name else {
        return fail("usage: gaff profile set <name> [--session <sid>]");
    };
    if !cfg.profiles.contains_key(&name) {
        return fail(&format!("unknown profile `{name}`"));
    }
    let Some(session) = session else {
        return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID");
    };
    if !std::io::stdin().is_terminal()
        && !cfg
            .transitions
            .clone()
            .unwrap_or_default()
            .agent_may_set(&name)
    {
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
/// Only a human may grant it. The boundary is structural, not a
/// terminal check: every command an agent runs passes through `gaff
/// hook` first, and the built-in guard there refuses this command. The
/// human's shell — including the harness's `!` shell, which attaches
/// no tty to stdin — has no hook, so this runs. A terminal check here
/// blocked exactly the channel it was meant to admit.
fn run_trust() -> ExitCode {
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
    let mut bad_guards = false;
    // Guards live only in the user config, and this is the subcommand
    // that reads it. Reporting handlers alone left the one blocking
    // feature unvalidated by the command documented to check it.
    match config::user_config_path().map(|p| config::load_user(&p)) {
        Some(Err(e)) => {
            eprintln!("gaff: {e}");
            bad_guards = true;
        }
        Some(Ok(Some(user))) => {
            for g in &user.guards {
                for p in g.problems() {
                    println!("FAIL {p}");
                    bad_guards = true;
                }
                for w in g.warnings() {
                    eprintln!("gaff: {w}");
                }
            }
            if !user.guards.is_empty() && !bad_guards {
                println!("ok   {} guard(s)", user.guards.len());
            }
        }
        _ => {}
    }
    let cfg = match crate::handler::load_checked() {
        Ok(cfg) => cfg,
        Err(e) => return fail(&e),
    };
    if cfg.handlers.is_empty() {
        println!("no handlers declared");
        return if bad_guards {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }
    let mut bad = false;
    // A name keys the pending marker and labels the output. Two
    // handlers under one name share a marker, so one of them may never
    // arm, and the model cannot tell their outputs apart.
    let mut seen = std::collections::HashSet::new();
    for h in &cfg.handlers {
        if !seen.insert(h.name.clone()) {
            bad = true;
            println!("FAIL duplicate handler name `{}`", h.name);
        }
    }
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
    let trusted = std::env::current_dir().is_ok_and(|d| crate::handler::is_trusted(&d));
    if !trusted {
        println!("note: this repo is not trusted, so no handler runs here. Run `gaff trust`.");
    }
    if bad || bad_guards {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
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
    // Only worth saying where the repo has a config of its own, which
    // is where the workflow plausibly belongs. On a repo with none, a
    // personal workflow is the whole point and the notice is noise for
    // the one person who is not making a mistake.
    let repo_has_config =
        std::env::current_dir().is_ok_and(|d| d.join(config::CONFIG_PATH).exists());
    if repo_has_config {
        for wf in cfg.github.iter().filter(|w| w.user) {
            eprintln!(
                "gaff: the workflow `{}` comes from your user config, so the file it writes is not reproducible from this repo alone, and a teammate's `gaff check` will call it an orphan. Declare it in the repo's config instead.",
                wf.name
            );
        }
    }
    // A workflow may reuse a git entry, so a broken entry breaks the
    // render. `init` skipped this check while `check` ran it, so init
    // wrote a step that runs nothing and only the later check objected,
    // after the useless workflow was committed.
    for entry in &cfg.git {
        for problem in entry.problems() {
            eprintln!("gaff: {problem}");
            bad = true;
        }
    }
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
    let mut problems: Vec<String> = cfg
        .git
        .iter()
        .flat_map(crate::githook::GitHook::problems)
        .collect();
    for wf in &cfg.github {
        problems.extend(wf.problems(&cfg.git));
    }
    if !problems.is_empty() {
        // A workflow can reuse a git entry, so a broken entry breaks
        // the workflow. Reporting only workflow problems let a CI job
        // that ran this command miss it.
        for p in &problems {
            eprintln!("gaff: {p}");
        }
        return ExitCode::FAILURE;
    }
    // The orphan scan runs even when nothing is declared. Returning
    // early here meant deleting the *last* workflow left its generated
    // file on disk and running in CI, while this command reported a
    // clean tree — the exact failure the orphan check exists to catch,
    // reached by deleting rather than renaming.
    let mut drifted = false;
    if cfg.github.is_empty() {
        let orphans = crate::ghworkflow::orphans(&cwd, &cfg.github);
        if orphans.is_empty() {
            println!("no workflows declared");
            return ExitCode::SUCCESS;
        }
        for orphan in orphans {
            println!("ORPHAN  {orphan} (generated by gaff, declared by nothing)");
        }
        return ExitCode::FAILURE;
    }
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
    if drifted {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `gaff allow <guard> [--session <sid>]` — let the next call a guard
/// would refuse through, once.
///
/// This is the human's release valve for a guard. What keeps it out of
/// an agent's hands is the built-in guard in the hook: every agent Bash
/// call passes through `gaff hook`, and this command is refused there.
/// The human's shell has no hook. That includes the harness's `!` shell,
/// which attaches no tty to stdin — so a terminal check here refused
/// the one channel it existed to admit.
fn run_allow(args: &[String]) -> ExitCode {
    let mut guard: Option<String> = None;
    let mut session_flag: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" => match it.next() {
                Some(v) => session_flag = Some(v.clone()),
                None => return fail("--session requires a value"),
            },
            other if guard.is_none() && !other.starts_with("--") => {
                guard = Some(other.to_string());
            }
            other => return fail(&format!("unexpected argument `{other}`")),
        }
    }
    let Some(guard) = guard else {
        return fail("usage: gaff allow <guard-name> [--session <sid>]");
    };
    let Some(session) = session_from(session_flag.as_deref()) else {
        return fail("no session. Pass --session or set CLAUDE_CODE_SESSION_ID.");
    };
    let Some(store) = resolve_store() else {
        return fail("no state directory. Set GAFF_STATE_DIR or HOME.");
    };
    // Name a guard that exists, so a typo does not read as a grant.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let known: Vec<String> = match config::load_layered(&cwd) {
        Loaded::Ok(cfg) | Loaded::Degraded(cfg) => {
            cfg.guards.iter().map(|g| g.name.clone()).collect()
        }
        _ => Vec::new(),
    };
    if !known.iter().any(|n| n == &guard) {
        return fail(&format!(
            "no guard named `{guard}`. The guards are: {}.",
            if known.is_empty() {
                "(none)".to_string()
            } else {
                known.join(", ")
            }
        ));
    }
    match store.write_allowance(&session, &guard) {
        Ok(()) => {
            println!("the next call `{guard}` would refuse is allowed, once.");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("cannot write the allowance: {e}")),
    }
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
        Loaded::Ok(cfg) => cfg,
        // `Degraded` means the repo config failed to parse and only the
        // user's layer stands. On the agent path that is right: gaff
        // warns and keeps going. Here it is not. The repo's checks are
        // exactly what cannot be read, so running the user's instead
        // and exiting 0 passes a commit the repo meant to gate.
        Loaded::Degraded(_) => {
            eprintln!(
                "gaff: {hook}: {} could not be read, so the checks it declares did not run. Fix the config, or remove the hook with `gaff init --git --uninstall`.",
                config::CONFIG_PATH
            );
            return ExitCode::FAILURE;
        }
        Loaded::Absent => {
            // The hook exists, so a config existed when it was
            // installed. Passing silently would run no check at all.
            let path = cwd.join(config::CONFIG_PATH);
            let detail = if path.exists() {
                "is empty"
            } else {
                "is missing"
            };
            eprintln!(
                "gaff: {hook}: {} {detail}, so no check ran. Restore it, or remove the hook with `gaff init --git --uninstall`.",
                config::CONFIG_PATH
            );
            return ExitCode::FAILURE;
        }
        Loaded::Broken(err) => {
            // A broken config must not silently pass a check that was
            // meant to run.
            return fail(&format!("{err}. Refusing to skip the {hook} checks."));
        }
    };
    // gaff's own script is installed for this hook, and the config
    // declares no entry for it. That only happens when the two have
    // drifted, and the effect is that a check the user installed stops
    // running in silence. Passing here is the same failure as passing
    // on an absent config.
    let stale = crate::githook::stale_installs(&cwd, &cfg.git);
    if stale.iter().any(|h| h == hook) {
        eprintln!(
            "gaff: {hook}: the hook is installed but the config declares no entry for it, so no check ran. Remove it with `gaff init --git --uninstall`, or restore the entry."
        );
        return ExitCode::FAILURE;
    }
    let code = crate::githook::run(&cwd, &cfg.git, hook, &args[1..]);
    u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from)
}

/// The message of the first guard that refuses this call.
///
/// This is the only path in gaff that leads to exit 2. Every failure
/// path, including a broken guard, still degrades, because a gaff
/// fault must never block a session.
fn guard_refusal(
    cfg: &config::Config,
    envelope: &crate::event::Envelope,
    adapter: &crate::adapter::Adapter,
) -> Option<(String, String)> {
    let tool = envelope.tool_name.as_deref()?;
    // The adapter owns the mapping from a payload shape to a named
    // field. A guard names a normalized tool and a field, so reading
    // the payload here would tie every guard to one host's schema.
    let value = |field: &str| {
        let found = (adapter.tool_field)(&envelope.raw, field);
        if found.is_none() {
            // A guard names this tool and gaff could not read the field
            // it matches on, so the guard did not run. Absent and
            // wrong-type were both silent; only the second warned.
            let raw_present = envelope
                .raw
                .get("tool_input")
                .and_then(|i| i.get(field))
                .is_some();
            let why = if raw_present {
                "is not text gaff can read"
            } else {
                "is not in the payload"
            };
            eprintln!(
                "gaff: a guard matches the `{field}` field of a {tool} call, and that field {why}, so the guard did not run."
            );
        }
        found
    };
    // The built-ins come first and no config can drop them.
    let mut guards = crate::guard::builtin();
    guards.extend(cfg.guards.iter().cloned());
    let hit = crate::guard::first_refusal(&guards, tool, &value)?;
    Some((
        hit.name.clone(),
        format!(
            "gaff: refused by the guard `{}`.\n\n{}\n\nIf the user has approved this specific call, they can run `!gaff allow {}` to let it through once.",
            hit.name, hit.message, hit.name
        ),
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
        for case in [
            vec!["--help"],
            vec!["-h"],
            vec!["help"],
            vec!["--version"],
            vec!["-V"],
            vec!["version"],
        ] {
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
        for cmd in COMMANDS {
            assert!(USAGE.contains(cmd), "the usage text omits `{cmd}`");
        }
    }

    #[test]
    fn no_arguments_is_a_usage_error_not_a_panic() {
        let code = run(&[]);
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}

#[cfg(test)]
mod prime_tests {
    use super::*;

    #[test]
    fn the_usage_text_opens_with_the_about_string() {
        assert!(
            USAGE.starts_with(&format!("gaff — {ABOUT}\n")),
            "USAGE line 1 is `gaff — {{ABOUT}}`"
        );
    }

    /// The dispatcher is a hand-written match, so this reads it: every
    /// command in COMMANDS has a `Some("<cmd>") =>` arm in `run`.
    #[test]
    fn every_command_dispatches() {
        let src = include_str!("cli.rs");
        let run_body = &src[src.find("pub fn run(").unwrap()..src.find("const USAGE").unwrap()];
        for cmd in COMMANDS {
            assert!(
                run_body.contains(&format!("Some(\"{cmd}\") =>")),
                "`run` does not dispatch `{cmd}`"
            );
        }
        let arms = run_body.matches("Some(\"").count();
        assert_eq!(
            arms,
            COMMANDS.len() + 2,
            "run has an arm COMMANDS does not list (version and help excepted)"
        );
    }

    #[test]
    fn prime_has_the_contract_shape() {
        let p = prime();
        let lines: Vec<&str> = p.lines().collect();
        assert!(p.len() <= 700, "prime is {} bytes; the cap is 700", p.len());
        assert_eq!(lines[0], "# gaff");
        assert_eq!(lines[1], ABOUT, "line 2 is the about string");
        assert!(
            p.ends_with('\n') && !p.ends_with("\n\n"),
            "one trailing newline"
        );
        assert!(!p.contains('\t'), "no tabs");
        assert!(!p.contains("[gaff:"), "no spoofable prefix");
        assert!(
            !p.chars().any(|c| c.is_control() && c != '\n'),
            "no control chars"
        );
        assert!(
            lines.iter().skip(1).all(|l| !l.starts_with('#')),
            "no headings below line 1"
        );
        assert!(
            lines
                .last()
                .unwrap()
                .starts_with("More: gaff --help; gaff docs")
        );
        for word in [
            "tisket",
            "zettel",
            "almanac",
            "mdstore",
            "Claude",
            "codex",
            "always",
            "never",
            "session start hook",
            "before you",
            "!gaff",
            "terminal",
        ] {
            assert!(!p.contains(word), "prime must not say {word:?}");
        }
    }

    /// Every `Commands:` line names a command `run` dispatches, and
    /// every `remind` line, with its placeholders filled, parses.
    #[test]
    fn every_prime_command_exists() {
        let p = prime();
        let start = p.find("Commands:\n").expect("a Commands: block") + "Commands:\n".len();
        let end = p.find("More:").expect("a More: line");
        let block = &p[start..end];
        assert!(block.lines().count() <= 7, "at most seven commands");
        for line in block.lines() {
            let mut words = line.split_whitespace();
            assert_eq!(words.next(), Some("gaff"), "{line:?} starts with the tool");
            let cmd = words.next().expect("a subcommand");
            assert!(
                COMMANDS.contains(&cmd),
                "{line:?}: `{cmd}` is not a command"
            );
            if cmd != "remind" {
                assert!(
                    words.next().is_none(),
                    "{line:?}: only remind takes arguments here"
                );
                continue;
            }
            // Expand `(a | b)` alternation, fill placeholders, and parse
            // each expansion. `[--flag <x>]` is tried present and absent.
            let rest: Vec<&str> = words.collect();
            let joined = rest.join(" ");
            let alternatives: Vec<String> = match (joined.find('('), joined.find(')')) {
                (Some(a), Some(b)) => joined[a + 1..b]
                    .split('|')
                    .map(|alt| format!("{}{}{}", &joined[..a], alt.trim(), &joined[b + 1..]))
                    .collect(),
                _ => vec![joined.clone()],
            };
            for alt in alternatives {
                // Optional groups are bracketed; try the line with every
                // group present and with every group absent.
                let mut present = String::new();
                let mut absent = String::new();
                let mut in_group = false;
                for tok in alt.split_whitespace() {
                    let opens = tok.starts_with('[');
                    let closes = tok.ends_with(']');
                    let bare = tok.trim_start_matches('[').trim_end_matches(']');
                    let bare = match bare {
                        "<text>" => "text",
                        "<n>" => "3",
                        "<id>" => "goal",
                        t => t,
                    };
                    present.push_str(bare);
                    present.push(' ');
                    if !(opens || in_group) {
                        absent.push_str(bare);
                        absent.push(' ');
                    }
                    in_group = (opens || in_group) && !closes;
                }
                for form in [present, absent] {
                    let args: Vec<String> = form.split_whitespace().map(str::to_string).collect();
                    let parsed = parse_remind(&args);
                    assert!(
                        parsed.is_ok(),
                        "{line:?} → `{form}` does not parse: {parsed:?}"
                    );
                }
            }
        }
    }
}
