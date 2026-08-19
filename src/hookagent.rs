//! Hook agents: `gaff run <name>` dispatches an agent through an agnostic
//! runner and maps its decision to an exit code.
//!
//! gaff names no runtime. It runs the configured context command, feeds
//! the output to the runner on stdin, reads a verdict marker from the
//! runner's output, and exits 0 on a pass. Every other outcome exits
//! [`BLOCK`], so a broken agent, a missing verdict, a runner error, or a
//! timeout refuses rather than admits.

use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::config::Agent;

/// The exit code a blocked or unanswerable run returns. It matches a
/// guard refusal: 2 is the only "blocked" code the ecosystem uses.
pub const BLOCK: i32 = 2;

/// The most context bytes fed to the runner. Over this, the run refuses
/// rather than review a truncation.
const CONTEXT_CAP: usize = 200 * 1024;

/// The most output bytes read from the runner before the verdict search.
const OUTPUT_CAP: usize = 1024 * 1024;

/// The verdict marker a runner's output carries. A reviewer ends with
/// `gaff-verdict: pass` or `gaff-verdict: fail[: reason]`. gaff reads it;
/// the runner needs no gaff-specific feature to print a line.
const MARKER: &str = "gaff-verdict:";

/// The agent's decision, read from the runner's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Pass,
    Fail(String),
    Missing,
}

/// Parse the agent's decision from the runner's output.
///
/// A line must begin with the marker to count, so an indented or quoted
/// marker inside prose is not a verdict. The last marker line wins, so a
/// reviewer that reasons first and concludes with a verdict is read by
/// its conclusion. No marker is [`Decision::Missing`], which the caller
/// fails closed.
#[must_use]
pub fn parse_decision(output: &str) -> Decision {
    let mut decision = Decision::Missing;
    for line in output.lines() {
        // Match at the true line start, not after leading whitespace, so
        // the verdict is a line the agent wrote as its verdict.
        let Some(rest) = strip_marker(line) else {
            continue;
        };
        let (word, reason) = match rest.trim().split_once(':') {
            Some((w, r)) => (w.trim(), r.trim()),
            None => (rest.trim(), ""),
        };
        decision = match word.to_ascii_lowercase().as_str() {
            "pass" => Decision::Pass,
            "fail" => Decision::Fail(reason.to_owned()),
            // An unrecognized word does not overwrite a real verdict; a
            // typo must not flip a fail to a missing and open the gate.
            _ => continue,
        };
    }
    decision
}

/// The text after a case-insensitive `gaff-verdict:` at the line start,
/// or `None`.
fn strip_marker(line: &str) -> Option<&str> {
    if line.len() < MARKER.len() {
        return None;
    }
    let (head, rest) = line.split_at(MARKER.len());
    head.eq_ignore_ascii_case(MARKER).then_some(rest)
}

/// Dispatch `agent` and return the process exit code. 0 only on a pass
/// verdict; every other path returns [`BLOCK`], so the gate fails closed.
#[must_use]
pub fn dispatch(agent: &Agent, cwd: &Path) -> i32 {
    let deadline = Duration::from_millis(agent.timeout_ms);

    // Engineer the context, if the agent declares a source.
    let context = if agent.context.is_empty() {
        None
    } else {
        match run_capturing(&agent.context, None, cwd, deadline, &[]) {
            Ok(c) if c.code == 0 && (c.capped || c.output.len() > CONTEXT_CAP) => {
                eprintln!(
                    "gaff: the context for `{}` is over the {CONTEXT_CAP}-byte cap.",
                    agent.name
                );
                return BLOCK;
            }
            Ok(c) if c.code == 0 => Some(c.output),
            Ok(c) => {
                eprintln!(
                    "gaff: the context command for `{}` exited {}, so the run is refused.",
                    agent.name, c.code
                );
                return BLOCK;
            }
            Err(reason) => {
                eprintln!(
                    "gaff: the context command for `{}` did not run: {reason}",
                    agent.name
                );
                return BLOCK;
            }
        }
    };

    // Dispatch the runner with the context on stdin.
    let runner = resolve_runner(agent);
    let mut env = vec![("GAFF_SESSION_ID".to_owned(), mint_session())];
    if let Some(profile) = &agent.profile {
        env.push(("GAFF_PROFILE".to_owned(), profile.clone()));
    }
    // The runner may name credentials it reads from the environment, such
    // as an API key. gaff clears the environment, so it adds these back by
    // name, and only if they are set.
    for key in &agent.env_passthrough {
        if let Ok(value) = std::env::var(key) {
            env.push((key.clone(), value));
        }
    }
    let captured = match run_capturing(
        &runner,
        context.as_deref().map(str::as_bytes),
        cwd,
        deadline,
        &env,
    ) {
        Ok(captured) => captured,
        Err(reason) => {
            eprintln!(
                "gaff: the runner for `{}` did not run: {reason}",
                agent.name
            );
            return BLOCK;
        }
    };
    if captured.code != 0 {
        eprintln!(
            "gaff: the runner for `{}` exited {}, so the run is refused.",
            agent.name, captured.code
        );
        return BLOCK;
    }
    // Capped output may have dropped a trailing verdict, so an earlier
    // `pass` must not stand in for one that was cut off.
    if captured.capped {
        eprintln!(
            "gaff: the output from `{}` hit the read cap, so the verdict cannot be trusted.",
            agent.name
        );
        return BLOCK;
    }

    match parse_decision(&captured.output) {
        Decision::Pass => 0,
        Decision::Fail(reason) => {
            let reason = if reason.is_empty() {
                "the agent refused the change".to_owned()
            } else {
                reason
            };
            eprintln!("gaff: `{}` refused: {reason}", agent.name);
            BLOCK
        }
        Decision::Missing => {
            eprintln!(
                "gaff: `{}` returned no verdict, so the run is refused.",
                agent.name
            );
            BLOCK
        }
    }
}

/// The runner command for `agent`, or the default derived from its name.
/// The default names kersh, but a config `runner` overrides it, so gaff
/// hardcodes no runtime.
fn resolve_runner(agent: &Agent) -> Vec<String> {
    if agent.runner.is_empty() {
        vec![
            "kersh".to_owned(),
            "run".to_owned(),
            agent.name.clone(),
            "--context-file".to_owned(),
            "-".to_owned(),
        ]
    } else {
        agent.runner.clone()
    }
}

/// A fresh session id for the run. gaff is an ordinary binary here, so a
/// process id and a high-resolution timestamp make a unique, valid id.
fn mint_session() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("agent-{}-{nanos}", std::process::id())
}

/// Spawn `argv`, feed `stdin`, and collect stdout within `timeout`,
/// killing the process group on the deadline. Returns the exit code and
/// the captured stdout.
fn run_capturing(
    argv: &[String],
    stdin: Option<&[u8]>,
    cwd: &Path,
    timeout: Duration,
    env: &[(String, String)],
) -> Result<Captured, String> {
    let program = argv.first().ok_or("an empty command")?;
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    // Clear the environment, then add back only a safe set. The context
    // command and the runner run in an untrusted repo, and a repo can set
    // a loader variable or a shadowing PATH entry through direnv or mise.
    // This is the boundary a handler's child holds too.
    cmd.env_clear();
    for key in crate::handler::ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.env("PATH", crate::handler::safe_path(cwd));
    for (key, value) in env {
        cmd.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|error| error.to_string())?;
    let pid = child.id();

    // Feed stdin on a detached thread, so a large context cannot deadlock
    // against a runner that has not begun to read.
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let owned = bytes.to_vec();
        std::thread::spawn(move || {
            use std::io::Write as _;
            let _ = pipe.write_all(&owned);
        });
    }

    let Some(mut stdout) = child.stdout.take() else {
        return Err("the runner was given no stdout pipe".to_owned());
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut capped = false;
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() >= OUTPUT_CAP {
                        buf.truncate(OUTPUT_CAP);
                        capped = true;
                        break;
                    }
                }
            }
        }
        let _ = tx.send((buf, capped));
    });

    let deadline = Instant::now() + timeout;
    let Ok((collected, capped)) = rx.recv_timeout(timeout) else {
        crate::handler::kill_group(pid);
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("it exceeded its {timeout:?} deadline"));
    };
    let (status, _killed) = crate::handler::wait_bounded(&mut child, pid, deadline, program);
    let code = status.and_then(|s| s.code()).unwrap_or(1);
    Ok(Captured {
        code,
        output: String::from_utf8_lossy(&collected).into_owned(),
        capped,
    })
}

/// The result of a captured run: the exit code, the stdout, and whether
/// the stdout hit the read cap. Capped output cannot be trusted for a
/// verdict, because a later `fail` may have been dropped.
struct Captured {
    code: i32,
    output: String,
    capped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_marker_is_a_pass() {
        assert_eq!(
            parse_decision("looks fine\ngaff-verdict: pass"),
            Decision::Pass
        );
    }

    #[test]
    fn a_fail_marker_carries_the_reason() {
        assert_eq!(
            parse_decision("gaff-verdict: fail: the retry loop never backs off"),
            Decision::Fail("the retry loop never backs off".to_owned())
        );
    }

    #[test]
    fn a_fail_without_a_reason_is_a_fail() {
        assert_eq!(
            parse_decision("gaff-verdict: fail"),
            Decision::Fail(String::new())
        );
    }

    #[test]
    fn no_marker_is_missing() {
        assert_eq!(
            parse_decision("I reviewed it and it is fine"),
            Decision::Missing
        );
    }

    #[test]
    fn the_last_marker_wins() {
        // A reviewer may think aloud before it concludes.
        assert_eq!(
            parse_decision("gaff-verdict: fail: first thought\ngaff-verdict: pass"),
            Decision::Pass
        );
    }

    #[test]
    fn the_marker_is_case_insensitive() {
        assert_eq!(parse_decision("GAFF-VERDICT: PASS"), Decision::Pass);
    }

    #[test]
    fn a_marker_mid_line_does_not_count() {
        // Only a line that starts with the marker is a verdict, so a
        // reviewer quoting the protocol does not flip its own result.
        assert_eq!(
            parse_decision("the format is `gaff-verdict: pass`"),
            Decision::Missing
        );
    }

    #[test]
    fn an_indented_marker_does_not_count() {
        // A verdict is a line the agent wrote as its verdict, at the line
        // start. An indented marker, such as one inside a quoted block a
        // diff carried, is not a verdict.
        assert_eq!(parse_decision("  gaff-verdict: pass"), Decision::Missing);
    }
}
