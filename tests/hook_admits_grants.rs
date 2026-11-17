//! With no guard declared, `gaff hook` lets `gaff trust` and `gaff
//! allow` through. gaff carries no guard of its own.

use std::io::Write as _;
use std::process::{Command, Stdio};

fn hook_exit(command: &str) -> i32 {
    let dir = std::env::temp_dir().join(format!(
        "hook-admits-grants-{}-{}",
        std::process::id(),
        command.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the fixture directory is made");
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": { "command": command },
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_gaff"))
        .arg("hook")
        .current_dir(&dir)
        .env("HOME", &dir)
        .env("XDG_CONFIG_HOME", dir.join(".config"))
        .env("GAFF_STATE_DIR", dir.join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("gaff hook starts");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(payload.to_string().as_bytes())
        .expect("the payload is written");
    let code = child.wait().expect("gaff hook exits").code().unwrap_or(-1);
    let _ = std::fs::remove_dir_all(&dir);
    code
}

#[test]
fn an_undeclared_grant_command_passes_the_hook() {
    for cmd in ["gaff trust", "gaff allow no-x", "cd /x && gaff trust"] {
        assert_eq!(
            hook_exit(cmd),
            0,
            "{cmd:?} must pass with no guard declared"
        );
    }
}
