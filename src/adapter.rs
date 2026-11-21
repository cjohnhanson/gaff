//! The adapter seam: the one place where a host's shape is known.
//!
//! Everything above this module works on the normalized [`Envelope`].
//! An adapter owns the host-specific facts: how a raw hook payload maps
//! to an envelope, how injected context is rendered back to the host,
//! which events gaff subscribes to, and where `gaff init` writes the
//! registration.
//!
//! A host reads injected context in its own shape, so `context_output`
//! renders it here. A refusal has no shape. Every host reads exit 2 as a
//! refusal and the child's stderr as the reason, and a host that blocks
//! on another code, or drops the child's stderr, breaks that contract.
//!
//! Four adapters ship. Claude Code is the host a person installs
//! against. Codex sends Claude Code's payload shape, has no batch event,
//! and fails a `Stop` hook that prints context. agy, the Antigravity
//! CLI, sends no event name, so its registration passes the event on
//! the command line. The generic host speaks gaff's own normalized
//! vocabulary, for a host such as an agent runner that calls `gaff hook`
//! itself. It reads only gaff's field names, so it is not a guess at a
//! real host's payload.
//!
//! A host that refuses a tool call on any non-zero exit sets
//! `failure_code` to 0, so a gaff fault still never blocks there.
//! gaff does not guess a payload shape. A guessed schema fails at run
//! time, inside a live session.
//!
//! # Adding an adapter
//!
//! Add a [`Adapter`] constant with the host's real field names, its
//! event names, its `context_output` shape, and, if it self-registers,
//! its settings path, taken from that host's documentation. Append it to
//! [`ADAPTERS`], never prepend, because `gaff init` uses the first entry
//! as its default. Give `sniff` a predicate that matches only that host's
//! payload. Nothing else in gaff changes.

use serde_json::Value;

use crate::event::{Envelope, Kind, SCHEMA_VERSION};

/// One agent host.
pub struct Adapter {
    /// The name for `--host` and `GAFF_HOST`.
    pub name: &'static str,
    /// The payload key that carries the event name. Each parse function
    /// reads its own key as a literal, and a contract test builds a
    /// sample payload from this field, so a second adapter's vocabulary
    /// is not assumed to be this host's.
    pub event_key: &'static str,
    /// Whether `gaff init` registers hooks for this host. A host that
    /// calls `gaff hook` directly needs no settings file, so it does not
    /// self-register and carries an empty settings path.
    pub self_registers: bool,
    /// Map a raw hook payload to the normalized envelope.
    pub parse: fn(Value) -> Envelope,
    /// Render injected context to this host's stdout shape. Claude Code
    /// wraps it in its hook JSON, and a generic host gets gaff's own
    /// normalized shape.
    pub context_output: fn(event: &str, context: &str) -> String,
    /// Recognize this host's payload by its shape. Detection prefers an
    /// explicit name; this is the fallback.
    pub sniff: fn(&Value) -> bool,
    /// Where `gaff init` registers the hooks, relative to the repo root.
    pub settings_path: &'static str,
    /// The host's other settings scopes that may also carry hooks, for
    /// `doctor`: the repo-shared file relative to the repo root, and the
    /// user file relative to `$HOME`.
    pub repo_settings_path: &'static str,
    pub user_settings_path: &'static str,
    /// The environment variable this host exports to a subprocess with
    /// the session id, so `gaff remind` and friends can find their
    /// session without a flag.
    pub session_env: &'static str,
    /// The events gaff subscribes to on this host.
    pub hook_events: &'static [&'static str],
    /// Read one tool-input field out of this host's raw payload.
    ///
    /// A guard names a normalized tool and a field. The mapping from
    /// that name to a payload key belongs to the adapter, because a
    /// host may nest its tool input anywhere.
    pub tool_field: fn(&Value, &str) -> Option<String>,
    /// The flush points this host can carry. A flush kind outside this
    /// set reaches gaff but has no context channel on this host, so gaff
    /// keeps the pending entries for the next flush point that does.
    /// Each kind here must come from an event in `hook_events`.
    pub flush_kinds: &'static [Kind],
    /// The settings key that holds the event map. Claude Code and Codex
    /// use `hooks`. agy keys each hook by a name, and gaff owns the one
    /// named `gaff`.
    pub hooks_key: &'static str,
    /// The command that one event's registration runs. A host that
    /// sends no event name, or a payload another host also sends, needs
    /// the command itself to say which host and event it is.
    pub registered_command: fn(event: &str, command: &str) -> String,
    /// One registration entry for an event, in this host's shape.
    pub hook_entry: fn(event: &str, command: &str) -> Value,
    /// The exit code for a gaff fault on the hook path. A host that
    /// reads exit 1 as non-blocking gets 1. A host that refuses a tool
    /// call on any non-zero code gets 0, because a gaff fault must never
    /// block a session.
    pub failure_code: u8,
    /// A line `gaff init` prints after it registers the hooks, for a
    /// step the host needs before the hooks run. Empty for none.
    pub install_note: &'static str,
    /// Whether the payload's working directory is the session's, in
    /// place of the hook process's own. agy runs a hook in the
    /// directory that holds its hook file, not in the workspace.
    pub cwd_from_payload: bool,
}

/// The events gaff needs on Claude Code: the prime and flush points,
/// plus the counted events.
pub const CLAUDE_CODE_EVENTS: &[&str] = &[
    "PostToolBatch",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SessionStart",
    "UserPromptSubmit",
    // Stop is a flush point and the one event a hold or a blocking
    // handler refuses. Without this line the host never calls gaff at a
    // stop, and every hold stays on disk unread.
    "Stop",
];

pub const CLAUDE_CODE: Adapter = Adapter {
    name: "claude-code",
    event_key: "hook_event_name",
    self_registers: true,
    parse: from_claude_code,
    context_output: claude_code_context_output,
    // Codex sends the same field names, plus `turn_id` on every event
    // inside a turn. A payload with `turn_id` is not Claude Code's.
    sniff: |json| json.get("hook_event_name").is_some() && json.get("turn_id").is_none(),
    settings_path: ".claude/settings.local.json",
    repo_settings_path: ".claude/settings.json",
    user_settings_path: ".claude/settings.json",
    session_env: "CLAUDE_CODE_SESSION_ID",
    hook_events: CLAUDE_CODE_EVENTS,
    tool_field: claude_code_tool_field,
    flush_kinds: ALL_FLUSH_KINDS,
    hooks_key: "hooks",
    registered_command: |_, command| command.to_string(),
    hook_entry: matcher_group_entry,
    failure_code: 1,
    install_note: "",
    cwd_from_payload: false,
};

/// Every flush point, for a host whose events carry them all.
const ALL_FLUSH_KINDS: &[Kind] = &[
    Kind::SessionStart,
    Kind::Prompt,
    Kind::ToolBatch,
    Kind::Stop,
];

/// A matcher group that runs one command, with no matcher, so it fires
/// for every tool. Claude Code and Codex share this shape.
fn matcher_group_entry(_event: &str, command: &str) -> Value {
    serde_json::json!({"hooks": [{"type": "command", "command": command}]})
}

/// The events a generic host subscribes to, in gaff's own normalized
/// vocabulary. A host that speaks the vocabulary needs no per-host names.
pub const GENERIC_EVENTS: &[&str] = &[
    "session_start",
    "prompt",
    "pre_tool_call",
    "tool_call",
    "tool_batch",
    "stop",
];

/// A host that speaks gaff's normalized vocabulary directly.
///
/// It reads the event from `gaff_event` and the rest from gaff's own
/// field names, so this is not a guess at any real host's payload. It
/// does not self-register, because a host such as kersh calls `gaff
/// hook` itself and needs no settings file. Like every host, it reads
/// exit 2 as a refusal and the child's stderr as the reason.
pub const GENERIC: Adapter = Adapter {
    name: "generic",
    event_key: "gaff_event",
    self_registers: false,
    parse: from_generic,
    context_output: generic_context_output,
    sniff: |json| json.get("gaff_event").is_some(),
    settings_path: "",
    repo_settings_path: "",
    user_settings_path: "",
    session_env: "GAFF_SESSION_ID",
    hook_events: GENERIC_EVENTS,
    tool_field: claude_code_tool_field,
    flush_kinds: ALL_FLUSH_KINDS,
    // A generic host keeps its own registration. These fields only keep
    // the adapter total.
    hooks_key: "hooks",
    registered_command: |_, command| command.to_string(),
    hook_entry: matcher_group_entry,
    failure_code: 1,
    install_note: "",
    cwd_from_payload: false,
};

/// The events gaff needs on Codex.
///
/// Source: <https://developers.openai.com/codex/hooks>, confirmed by
/// real `codex exec` runs with codex-cli 0.153.4. Codex has no batch
/// event. `Stop` is registered for holds: exit 2 on `Stop` is a
/// documented block, and Codex feeds the reason back as the next
/// prompt. `Stop` is not a flush point on Codex, because Codex fails a
/// `Stop` hook that prints `hookSpecificOutput`.
pub const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
];

pub const CODEX: Adapter = Adapter {
    name: "codex",
    event_key: "hook_event_name",
    self_registers: true,
    // Codex sends Claude Code's field names and event names, and reads
    // context from the same `hookSpecificOutput` JSON.
    parse: from_codex,
    context_output: claude_code_context_output,
    // `SessionStart` has no `turn_id`, so this matches only events
    // inside a turn. The registered command names the host for the rest.
    sniff: |json| json.get("hook_event_name").is_some() && json.get("turn_id").is_some(),
    // Codex loads `.codex/hooks.json` from a trusted project, and the
    // user file from `$CODEX_HOME/hooks.json`. It has no local scope.
    settings_path: ".codex/hooks.json",
    repo_settings_path: ".codex/hooks.json",
    user_settings_path: ".codex/hooks.json",
    // Codex exports the session id to a tool's process as
    // `CODEX_THREAD_ID`, equal to the hook payload's `session_id`.
    session_env: "CODEX_THREAD_ID",
    hook_events: CODEX_EVENTS,
    tool_field: codex_tool_field,
    flush_kinds: &[Kind::SessionStart, Kind::Prompt],
    hooks_key: "hooks",
    // A `SessionStart` payload has no `turn_id`, so the shape alone
    // reads as Claude Code. The command names the host instead.
    registered_command: |_, command| format!("{command} --host codex"),
    hook_entry: codex_hook_entry,
    failure_code: 1,
    install_note: "Codex runs a project hook only after you trust it. Open codex in this repo and approve the gaff hooks in /hooks.",
    // Codex runs a hook in the session's working directory.
    cwd_from_payload: false,
};

/// Codex's matcher group, with Codex's own size limit turned off.
///
/// Codex saves `additionalContext` over about 2,500 tokens to a file and
/// gives the model a preview. A session-start flush of real sections
/// sits near that size, and a preview would hide the sections. gaff
/// already caps every flush at `max_inject_bytes`, so that cap is the
/// one limit, as on the other hosts. `0` passes the text through. The
/// limit goes only on the flush events: Codex warns on every session
/// about a limit set on `Stop`.
fn codex_hook_entry(event: &str, command: &str) -> Value {
    match event {
        "SessionStart" | "UserPromptSubmit" => serde_json::json!({"hooks": [{
            "type": "command",
            "command": command,
            "additionalContextLimit": 0,
        }]}),
        _ => matcher_group_entry(event, command),
    }
}

/// Codex sends Claude Code's payload, with one difference that reaches a
/// guard: a file edit arrives as `apply_patch`. Codex's own matcher
/// treats `apply_patch` as `Edit` and `Write`. gaff names it `Edit`, so
/// a guard on `Edit` reaches it, and `codex_tool_field` reads the paths
/// out of the patch.
fn from_codex(json: Value) -> Envelope {
    let mut env = from_claude_code(json);
    if env.tool_name.as_deref() == Some("apply_patch") {
        env.tool_name = Some("Edit".to_string());
    }
    env
}

/// Codex's tool input is Claude Code's, except for a patch. A patch has
/// no `file_path`, so the field is the path of each file the patch
/// adds, updates, or deletes, one per line, in patch order.
fn codex_tool_field(raw: &Value, field: &str) -> Option<String> {
    if field == "file_path" && raw.get("tool_name").and_then(Value::as_str) == Some("apply_patch") {
        let patch = raw.pointer("/tool_input/command")?.as_str()?;
        let paths: Vec<&str> = patch
            .lines()
            .filter_map(|line| {
                ["*** Add File: ", "*** Update File: ", "*** Delete File: "]
                    .iter()
                    .find_map(|head| line.strip_prefix(head))
            })
            .collect();
        return (!paths.is_empty()).then(|| paths.join("\n"));
    }
    claude_code_tool_field(raw, field)
}

/// The events gaff needs on agy, the Antigravity CLI.
///
/// Source: <https://antigravity.google/docs/hooks>, confirmed by real
/// `agy -p` runs with agy 1.4.8. `PreInvocation` fires before each
/// model call, and gaff reads it as a prompt or a tool batch. agy reads
/// a refusal at `Stop` only as a JSON decision, so gaff does not
/// register `Stop`, and a hold does not run on agy.
pub const AGY_EVENTS: &[&str] = &["SessionStart", "PreToolUse", "PostToolUse", "PreInvocation"];

pub const AGY: Adapter = Adapter {
    name: "agy",
    // agy sends no event name. `gaff hook --event` writes it under this
    // key, so the key is gaff's own and not an agy field.
    event_key: "hookEventName",
    self_registers: true,
    parse: from_agy,
    context_output: agy_context_output,
    sniff: |json| json.get("conversationId").is_some(),
    settings_path: ".agents/hooks.json",
    repo_settings_path: ".agents/hooks.json",
    user_settings_path: ".gemini/config/hooks.json",
    session_env: "ANTIGRAVITY_CONVERSATION_ID",
    hook_events: AGY_EVENTS,
    tool_field: agy_tool_field,
    flush_kinds: &[Kind::SessionStart, Kind::Prompt, Kind::ToolBatch],
    hooks_key: "gaff",
    registered_command: |event, command| format!("{command} --host agy --event {event}"),
    hook_entry: agy_hook_entry,
    // agy refuses a tool call on any non-zero exit from `PreToolUse`.
    failure_code: 0,
    install_note: "",
    // A real run showed a workspace hook's working directory is
    // `.agents`, so the repo's `.gaff/` is found from the workspace path.
    cwd_from_payload: true,
};

/// agy nests a tool event's handlers under a matcher. Every other event
/// takes the handler directly.
fn agy_hook_entry(event: &str, command: &str) -> Value {
    let handler = serde_json::json!({"type": "command", "command": command});
    match event {
        "PreToolUse" | "PostToolUse" => serde_json::json!({"matcher": "*", "hooks": [handler]}),
        _ => handler,
    }
}

/// agy reads injected context as steps added to the trajectory. A
/// `systemMessage` step stays in the conversation, as context does on
/// the other hosts.
fn agy_context_output(_event: &str, context: &str) -> String {
    serde_json::json!({
        "injectSteps": [{"systemMessage": {"systemMessage": context}}]
    })
    .to_string()
}

/// agy's tool names, mapped onto the names a guard uses. A name not in
/// this list stays as agy sends it, and a guard can name it directly.
fn agy_tool_name(name: &str) -> &str {
    match name {
        "run_command" => "Bash",
        "read_file" => "Read",
        "write_file" => "Write",
        "edit_file" => "Edit",
        other => other,
    }
}

/// agy puts the tool input under `toolCall.args`, with its own keys.
fn agy_tool_field(raw: &Value, field: &str) -> Option<String> {
    let key = match field {
        "command" => "CommandLine",
        "file_path" => "Path",
        _ => return None,
    };
    raw.pointer("/toolCall/args")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Build an envelope from an agy hook payload.
///
/// The event name comes from `hookEventName`, which `gaff hook --event`
/// adds. The session is `conversationId`, and the working directory is
/// the first workspace path. A tool event gets a `tool_use_id` built
/// from `stepIdx`, because the counter keys a tool call on that field
/// and agy sends no id of its own.
fn from_agy(mut json: Value) -> Envelope {
    let get = |json: &Value, key: &str| json.get(key).and_then(Value::as_str).map(str::to_string);
    let event = get(&json, "hookEventName").unwrap_or_else(|| "Unknown".to_string());
    let kind = match event.as_str() {
        "SessionStart" => Kind::SessionStart,
        "PreToolUse" => Kind::PreToolCall,
        "PostToolUse" => Kind::ToolCall,
        // The first model call of a turn follows the user's prompt.
        // Every later one follows a batch of tool results.
        "PreInvocation" => match json.get("invocationNum").and_then(Value::as_u64) {
            Some(0) => Kind::Prompt,
            Some(_) => Kind::ToolBatch,
            None => Kind::Other(event.clone()),
        },
        other => Kind::Other(other.to_string()),
    };
    if let (Some(step), Some(map)) = (
        json.get("stepIdx").and_then(Value::as_u64),
        json.as_object_mut(),
    ) {
        map.entry("tool_use_id")
            .or_insert_with(|| Value::String(format!("step-{step}")));
    }
    Envelope {
        gaff_schema: SCHEMA_VERSION,
        kind,
        session_id: safe_session(get(&json, "conversationId")),
        cwd: json
            .pointer("/workspacePaths/0")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_name: json
            .pointer("/toolCall/name")
            .and_then(Value::as_str)
            .map(|n| agy_tool_name(n).to_string()),
        event,
        raw: json,
    }
}

/// Drop an unsafe session id at the boundary. The id names a state
/// directory, and it arrives from the host payload.
fn safe_session(id: Option<String>) -> Option<String> {
    id.filter(|id| {
        let ok = crate::state::valid_session_id(id);
        if !ok {
            eprintln!("gaff: refusing an unsafe session id. Passing through.");
        }
        ok
    })
}

/// Claude Code reads injected context from this exact JSON on stdout.
///
/// The bytes must not drift: the missouri suites parse this field, and a
/// byte-exact unit test pins it. `serde_json` keeps the key order because
/// the crate enables `preserve_order`.
fn claude_code_context_output(event: &str, context: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": context,
        }
    })
    .to_string()
}

/// A generic host reads gaff's own normalized shape: the event and the
/// context text. It is self-delimiting and leaves room for a later field.
fn generic_context_output(event: &str, context: &str) -> String {
    serde_json::json!({ "event": event, "context": context }).to_string()
}

/// Claude Code puts the tool input under `tool_input`.
///
/// A field arrives as a string or as an argv array. Every element of an
/// array must be a string. A dropped element makes a command line the
/// host never sent, and a guard does not match that line.
fn claude_code_tool_field(raw: &Value, field: &str) -> Option<String> {
    match raw.get("tool_input").and_then(|i| i.get(field))? {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let parts: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
            parts.map(|p| {
                p.iter()
                    .map(|a| shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        }
        _ => None,
    }
}

/// Render one argv element as the shell text that produces it.
///
/// A guard pattern reads a command line, and a host that sends argv has
/// none. Joining the elements with a space invents one, and an argument
/// holding a space then reads as two, which is a different command from
/// the one the host is about to run.
fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Every implemented adapter.
///
/// `CLAUDE_CODE` stays first: `gaff init` with no `--host` uses the first
/// entry as the default target, and Claude Code is the host a person
/// installs against. A new adapter is appended, never prepended.
pub const ADAPTERS: &[&Adapter] = &[&CLAUDE_CODE, &GENERIC, &CODEX, &AGY];

/// Build an envelope from a Claude Code hook payload.
fn from_claude_code(json: Value) -> Envelope {
    let get = |key: &str| {
        json.get(key)
            .and_then(Value::as_str)
            .map(std::string::ToString::to_string)
    };
    let event = get("hook_event_name").unwrap_or_else(|| "Unknown".to_string());
    Envelope {
        gaff_schema: SCHEMA_VERSION,
        kind: claude_code_kind(&event),
        event,
        // Drop an unsafe id at the boundary. The id names a state
        // directory, and it arrives from the host payload.
        session_id: get("session_id").filter(|id| {
            let ok = crate::state::valid_session_id(id);
            if !ok {
                eprintln!("gaff: refusing an unsafe session id. Passing through.");
            }
            ok
        }),
        cwd: get("cwd"),
        tool_name: get("tool_name"),
        raw: json,
    }
}

/// Build an envelope from a generic host's payload.
///
/// The event is already a normalized name, so `Kind::parse` maps it. The
/// rest reads gaff's own field names. Nothing is guessed about a real
/// host: a host opts into this shape by naming itself `generic`.
fn from_generic(json: Value) -> Envelope {
    let get = |key: &str| {
        json.get(key)
            .and_then(Value::as_str)
            .map(std::string::ToString::to_string)
    };
    let event = get("gaff_event").unwrap_or_else(|| "unknown".to_string());
    Envelope {
        gaff_schema: SCHEMA_VERSION,
        kind: Kind::parse(&event),
        event,
        session_id: get("session_id").filter(|id| {
            let ok = crate::state::valid_session_id(id);
            if !ok {
                eprintln!("gaff: refusing an unsafe session id. Passing through.");
            }
            ok
        }),
        cwd: get("cwd"),
        tool_name: get("tool_name"),
        raw: json,
    }
}

/// Map a Claude Code event name onto the normalized set.
///
/// This mapping is the adapter's job. Nothing above the adapter knows
/// that this host calls a prompt `UserPromptSubmit`.
fn claude_code_kind(event: &str) -> Kind {
    match event {
        "SessionStart" => Kind::SessionStart,
        "UserPromptSubmit" => Kind::Prompt,
        "PreToolUse" => Kind::PreToolCall,
        "PostToolUse" | "PostToolUseFailure" => Kind::ToolCall,
        "PostToolBatch" => Kind::ToolBatch,
        "Stop" => Kind::Stop,
        other => Kind::Other(other.to_string()),
    }
}

/// The session id for a command run inside a session: the flag, then
/// gaff's own `GAFF_SESSION_ID`, then whichever host variable is set.
/// Nothing above the adapter knows what any host calls it.
#[must_use]
pub fn session_from_env(flag: Option<&str>) -> Option<String> {
    if let Some(f) = flag {
        return Some(f.to_string());
    }
    if let Ok(s) = std::env::var("GAFF_SESSION_ID")
        && !s.is_empty()
    {
        return Some(s);
    }
    ADAPTERS
        .iter()
        .find_map(|a| std::env::var(a.session_env).ok().filter(|s| !s.is_empty()))
}

/// The one-line hint for a missing session, naming every host's variable.
#[must_use]
pub fn session_hint() -> String {
    // Name only a self-registering host's variable. A generic host uses
    // GAFF_SESSION_ID, which the sentence already names, so listing it
    // again reads as a duplicate.
    let hosts: Vec<String> = ADAPTERS
        .iter()
        .filter(|a| a.self_registers)
        .map(|a| format!("{} for {}", a.session_env, a.name))
        .collect();
    format!(
        "no session. Pass --session, or set GAFF_SESSION_ID or the host's variable ({}).",
        hosts.join(", ")
    )
}

/// Look up an adapter by name.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().copied().find(|a| a.name == name)
}

/// The name of every implemented adapter, for an error message.
#[must_use]
pub fn names() -> String {
    ADAPTERS
        .iter()
        .map(|a| a.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Choose the adapter for a payload. An explicit name wins. Otherwise
/// the payload shape decides. An unrecognized shape falls back to the
/// default adapter, because gaff degrades rather than blocks.
#[must_use]
pub fn detect(explicit: Option<&str>, payload: &Value) -> &'static Adapter {
    if let Some(name) = explicit {
        if let Some(found) = by_name(name) {
            return found;
        }
        // Say so on stderr. A typo in GAFF_HOST reads as a working
        // setting while gaff parses another host's payload shape.
        eprintln!(
            "gaff: no adapter named `{name}` (implemented: {}). Detecting the host from the payload instead.",
            ADAPTERS
                .iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    ADAPTERS
        .iter()
        .copied()
        .find(|a| (a.sniff)(payload))
        .unwrap_or(&CLAUDE_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_explicit_name_wins_over_the_shape() {
        let payload = json!({"hook_event_name": "SessionStart"});
        assert_eq!(detect(Some("claude-code"), &payload).name, "claude-code");
    }

    #[test]
    fn an_unknown_name_falls_back_to_the_shape() {
        // gaff degrades; an unknown host name never blocks a session.
        let payload = json!({"hook_event_name": "SessionStart"});
        assert_eq!(detect(Some("nope"), &payload).name, "claude-code");
    }

    #[test]
    fn the_shape_selects_the_adapter() {
        let payload = json!({"hook_event_name": "UserPromptSubmit"});
        let adapter = detect(None, &payload);
        assert_eq!(adapter.name, "claude-code");
        let env = (adapter.parse)(payload);
        assert_eq!(env.event, "UserPromptSubmit");
    }

    #[test]
    fn every_adapter_declares_events_and_a_settings_path_if_it_registers() {
        for adapter in ADAPTERS {
            assert!(!adapter.hook_events.is_empty(), "{}", adapter.name);
            assert!(by_name(adapter.name).is_some(), "{}", adapter.name);
            // A self-registering adapter needs a place to write hooks. A
            // generic host calls gaff directly and needs none.
            if adapter.self_registers {
                assert!(!adapter.settings_path.is_empty(), "{}", adapter.name);
            } else {
                assert!(adapter.settings_path.is_empty(), "{}", adapter.name);
            }
            assert!(!adapter.session_env.is_empty(), "{}", adapter.name);
        }
    }

    #[test]
    fn the_context_output_shapes_are_stable() {
        // Claude Code's bytes are pinned: the missouri suites parse this
        // field and a byte change would silently reshape a live session.
        assert_eq!(
            (CLAUDE_CODE.context_output)("SessionStart", "hello"),
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"hello"}}"#
        );
        assert_eq!(
            (GENERIC.context_output)("session_start", "hello"),
            r#"{"event":"session_start","context":"hello"}"#
        );
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;
    use crate::event::Kind;
    use serde_json::json;

    #[test]
    fn the_adapter_maps_host_names_onto_the_normalized_set() {
        // Nothing above the adapter should ever see these host strings.
        for (host, want) in [
            ("SessionStart", Kind::SessionStart),
            ("UserPromptSubmit", Kind::Prompt),
            ("PostToolUse", Kind::ToolCall),
            ("PostToolUseFailure", Kind::ToolCall),
            ("PostToolBatch", Kind::ToolBatch),
            ("Stop", Kind::Stop),
        ] {
            let env = (CLAUDE_CODE.parse)(json!({"hook_event_name": host, "session_id": "s"}));
            assert_eq!(env.kind, want, "{host}");
            assert_eq!(env.event, host, "the host name stays for the log");
        }
    }

    #[test]
    fn an_unmapped_host_event_stays_first_class_and_permits_nothing() {
        let env = (CLAUDE_CODE.parse)(json!({"hook_event_name": "SomeFutureEvent"}));
        assert_eq!(env.kind, Kind::Other("SomeFutureEvent".to_string()));
        assert!(
            !env.kind.is_flush(),
            "an unknown event is never a flush point"
        );
        assert!(!env.capability().verified);
    }

    #[test]
    fn the_flush_points_are_the_cross_agent_set() {
        assert!(Kind::SessionStart.is_flush());
        assert!(Kind::Prompt.is_flush());
        assert!(Kind::ToolBatch.is_flush());
        // Stop is the last event of a turn, and its sink is verified.
        assert!(Kind::Stop.is_flush());
        // A tool call's context rides the tool result, never the framing.
        assert!(!Kind::ToolCall.is_flush());
        assert!(!Kind::PreToolCall.is_flush());
    }

    #[test]
    fn a_normalized_name_round_trips_through_a_config() {
        for k in [
            Kind::SessionStart,
            Kind::Prompt,
            Kind::ToolCall,
            Kind::ToolBatch,
            Kind::Stop,
        ] {
            assert_eq!(Kind::parse(k.as_str()), k);
        }
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    /// Every flush point gaff acts on must be an event the adapter
    /// subscribes to. A flush point that is not subscribed is dead on
    /// that host: `gaff init` never registers it, so the host never
    /// calls gaff there.
    #[test]
    fn every_flush_point_is_a_subscribed_event() {
        for adapter in ADAPTERS {
            // An event can map to more than one kind by its payload, as
            // agy's `PreInvocation` does by `invocationNum`, so each
            // event is parsed with both values.
            let subscribed_kinds: Vec<crate::event::Kind> = adapter
                .hook_events
                .iter()
                .flat_map(|event| {
                    [0, 1].map(|n| {
                        let payload = serde_json::json!({
                            adapter.event_key: event,
                            "session_id": "s",
                            "invocationNum": n,
                        });
                        (adapter.parse)(payload).kind
                    })
                })
                .collect();
            assert!(
                !adapter.flush_kinds.is_empty(),
                "adapter `{}` declares no flush point",
                adapter.name
            );
            for kind in adapter.flush_kinds {
                assert!(kind.is_flush(), "{kind:?} is a flush point");
                assert!(
                    subscribed_kinds.contains(kind),
                    "adapter `{}` flushes on {kind:?} but never subscribes to it, so that flush point is dead on this host",
                    adapter.name
                );
            }
        }
    }

    /// The full flush set on the hosts whose events carry it. A host
    /// that loses one must say why in its adapter.
    #[test]
    fn the_flush_sets_match_what_each_host_carries() {
        assert_eq!(CLAUDE_CODE.flush_kinds, ALL_FLUSH_KINDS);
        assert_eq!(GENERIC.flush_kinds, ALL_FLUSH_KINDS);
        assert_eq!(CODEX.flush_kinds, &[Kind::SessionStart, Kind::Prompt]);
        assert_eq!(
            AGY.flush_kinds,
            &[Kind::SessionStart, Kind::Prompt, Kind::ToolBatch]
        );
    }

    /// Guards gate on `Kind::PreToolCall`, and an unrecognized event
    /// falls through to `Kind::Other`. An adapter whose pre-tool event
    /// is never mapped gets zero guards and says nothing, so this test
    /// asserts the mapping.
    #[test]
    fn every_adapter_maps_some_event_to_a_pre_tool_call() {
        for adapter in ADAPTERS {
            let mapped = adapter.hook_events.iter().any(|event| {
                let payload = serde_json::json!({
                    adapter.event_key: event,
                    "session_id": "s",
                    "tool_name": "Bash",
                    "tool_input": {"command": "x"},
                });
                (adapter.parse)(payload).kind == crate::event::Kind::PreToolCall
            });
            assert!(
                mapped,
                "adapter `{}` subscribes to no event that becomes a pre-tool call, so no guard can ever fire on it",
                adapter.name
            );
        }
    }

    /// A guard names a normalized tool and a field, and the adapter maps
    /// that name onto its host's payload. An adapter that cannot read
    /// the field disarms every guard while `check` and `doctor` still
    /// report them active.
    #[test]
    fn every_adapter_can_read_a_tool_field() {
        for adapter in ADAPTERS {
            // Each host nests the tool input in its own shape.
            let payload = if adapter.name == AGY.name {
                serde_json::json!({
                    "conversationId": "s",
                    "toolCall": {"name": "run_command", "args": {"CommandLine": "git status"}},
                })
            } else {
                serde_json::json!({
                    "hook_event_name": adapter.hook_events[0],
                    "session_id": "s",
                    "tool_name": "Bash",
                    "tool_input": {"command": "git status"},
                })
            };
            assert_eq!(
                (adapter.tool_field)(&payload, "command").as_deref(),
                Some("git status"),
                "adapter `{}` cannot read the field a guard names",
                adapter.name
            );
            assert_eq!(
                (adapter.tool_field)(&payload, "file_path"),
                None,
                "adapter `{}` invented a field the payload lacks",
                adapter.name
            );
        }
    }

    #[test]
    fn an_argv_shaped_field_reads_as_the_command_line_it_stands_for() {
        // agy sends `CommandLine` as one string, so only the hosts that
        // read `tool_input` can send argv.
        for adapter in [&CLAUDE_CODE, &GENERIC, &CODEX] {
            let payload = serde_json::json!({
                "tool_input": {"command": ["git", "add", "release notes.md"]},
            });
            assert_eq!(
                (adapter.tool_field)(&payload, "command").as_deref(),
                Some("git add 'release notes.md'"),
                "adapter `{}` lost the quoting an argv element needs",
                adapter.name
            );
            // A non-string element means the value is not a command
            // line, and a line built from the rest passes the guard.
            let mixed = serde_json::json!({"tool_input": {"command": ["git", 7]}});
            assert_eq!((adapter.tool_field)(&mixed, "command"), None);
        }
    }

    /// The command each registration runs must still read as `gaff
    /// hook`, or `doctor` and `uninstall` lose it.
    #[test]
    fn every_registered_command_still_runs_gaff_hook() {
        for adapter in ADAPTERS {
            for event in adapter.hook_events {
                let cmd = (adapter.registered_command)(event, "gaff hook");
                assert!(cmd.starts_with("gaff hook"), "{}: {cmd}", adapter.name);
            }
        }
    }
}

/// Tests built from payloads that real host sessions sent during QA.
/// Paths are shortened, and every field name and value shape is as the
/// host sent it.
#[cfg(test)]
mod host_payload_tests {
    use super::*;
    use serde_json::json;

    /// `codex exec`, codex-cli 0.153.4.
    fn codex_pre_tool_use() -> Value {
        json!({
            "session_id": "01a1fc73-9253-7280-b888-d256837fd62e",
            "turn_id": "01a1fc73-935c-7393-9cff-bc42b423f180",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/private/tmp/gaffqa",
            "hook_event_name": "PreToolUse",
            "model": "gpt-6-astra",
            "permission_mode": "default",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi"},
            "tool_use_id": "exec-455cadc1-c408-49c3-81c0-3b143653de83"
        })
    }

    /// `codex exec`: `SessionStart` carries no `turn_id`.
    fn codex_session_start() -> Value {
        json!({
            "session_id": "01a1fc73-9253-7280-b888-d256837fd62e",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/private/tmp/gaffqa",
            "hook_event_name": "SessionStart",
            "model": "gpt-6-astra",
            "permission_mode": "default",
            "source": "startup"
        })
    }

    /// `agy -p`, agy 1.4.8, after `gaff hook --event PreToolUse`
    /// added `hookEventName`.
    fn agy_pre_tool_use() -> Value {
        json!({
            "hookEventName": "PreToolUse",
            "artifactDirectoryPath": "/tmp/brain/69348f19-f6b5-44b1-8eab-8c19e937f6d0",
            "conversationId": "69348f19-f6b5-44b1-8eab-8c19e937f6d0",
            "lastUserInput": "Run the shell command 'echo hi' and then reply DONE.",
            "modelName": "gemini-4-argon-medium",
            "stepIdx": 2,
            "toolCall": {
                "args": {"CommandLine": "echo hi", "WaitMsBeforeAsync": 5000, "toolSummary": "Echo hi"},
                "name": "run_command"
            },
            "transcriptPath": "/tmp/brain/transcript_full.jsonl",
            "workspacePaths": ["/tmp/gaffqa"]
        })
    }

    /// `agy -p`: the same call's `PostToolUse` carries `error`.
    fn agy_post_tool_use() -> Value {
        json!({
            "hookEventName": "PostToolUse",
            "conversationId": "d7ef1b97-fbf0-4bb4-87f7-0f49c927aefb",
            "error": "",
            "stepIdx": 6,
            "toolCall": {"args": {"Path": "/tmp/gaffqa/note.txt"}, "name": "read_file"},
            "workspacePaths": ["/tmp/gaffqa"]
        })
    }

    /// `agy -p`: `PreInvocation` before the first and the second model
    /// call of one turn.
    fn agy_pre_invocation(n: u64) -> Value {
        json!({
            "hookEventName": "PreInvocation",
            "conversationId": "d7ef1b97-fbf0-4bb4-87f7-0f49c927aefb",
            "initialNumSteps": 1 + 3 * n,
            "invocationNum": n,
            "lastUserInput": "Read note.txt and reply with its content.",
            "modelName": "gemini-4-argon-medium",
            "workspacePaths": ["/tmp/gaffqa"]
        })
    }

    #[test]
    fn a_codex_payload_selects_codex_by_shape() {
        assert_eq!(detect(None, &codex_pre_tool_use()).name, "codex");
        // A Claude Code payload has no `turn_id`, so it stays Claude Code.
        let claude = json!({"hook_event_name": "PreToolUse", "session_id": "s"});
        assert_eq!(detect(None, &claude).name, "claude-code");
        // `SessionStart` has no `turn_id`; the registered command names
        // the host, and an explicit name wins.
        assert_eq!(detect(Some("codex"), &codex_session_start()).name, "codex");
    }

    #[test]
    fn a_codex_payload_parses_to_the_normalized_envelope() {
        let env = (CODEX.parse)(codex_pre_tool_use());
        assert_eq!(env.kind, Kind::PreToolCall);
        assert_eq!(
            env.session_id.as_deref(),
            Some("01a1fc73-9253-7280-b888-d256837fd62e")
        );
        assert_eq!(env.cwd.as_deref(), Some("/private/tmp/gaffqa"));
        assert_eq!(env.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            (CODEX.tool_field)(&env.raw, "command").as_deref(),
            Some("echo hi")
        );
        assert_eq!(
            (CODEX.parse)(codex_session_start()).kind,
            Kind::SessionStart
        );
        for (event, kind) in [
            ("UserPromptSubmit", Kind::Prompt),
            ("PostToolUse", Kind::ToolCall),
            ("Stop", Kind::Stop),
        ] {
            let mut p = codex_pre_tool_use();
            p["hook_event_name"] = json!(event);
            assert_eq!((CODEX.parse)(p).kind, kind, "{event}");
        }
    }

    #[test]
    fn a_codex_patch_reaches_an_edit_guard_with_its_paths() {
        // Real `codex exec` payload for an `apply_patch` edit.
        let mut p = codex_pre_tool_use();
        p["tool_name"] = json!("apply_patch");
        p["tool_input"] = json!({
            "command": "*** Begin Patch\n*** Update File: /private/tmp/gaffqa/patched.txt\n@@\n-alpha\n+beta\n*** Add File: /private/tmp/gaffqa/new.txt\n+x\n*** End Patch"
        });
        let env = (CODEX.parse)(p);
        assert_eq!(env.tool_name.as_deref(), Some("Edit"));
        assert_eq!(
            (CODEX.tool_field)(&env.raw, "file_path").as_deref(),
            Some("/private/tmp/gaffqa/patched.txt\n/private/tmp/gaffqa/new.txt")
        );
        // A shell call has no file path, patch or not.
        assert_eq!((CODEX.tool_field)(&codex_pre_tool_use(), "file_path"), None);
    }

    #[test]
    fn codex_reads_context_in_the_hook_specific_shape() {
        // A real `codex exec` run quoted back a marker sent in exactly
        // this JSON on `SessionStart` and `UserPromptSubmit`.
        assert_eq!(
            (CODEX.context_output)("SessionStart", "hello"),
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"hello"}}"#
        );
    }

    #[test]
    fn codex_registers_the_host_on_the_command_and_nests_a_matcher_group() {
        assert_eq!(
            (CODEX.registered_command)("SessionStart", "gaff hook"),
            "gaff hook --host codex"
        );
        assert_eq!(
            (CODEX.hook_entry)("SessionStart", "gaff hook --host codex"),
            json!({"hooks": [{
                "type": "command",
                "command": "gaff hook --host codex",
                "additionalContextLimit": 0
            }]})
        );
        // A real run warned on a limit set on `Stop`.
        assert_eq!(
            (CODEX.hook_entry)("Stop", "c"),
            json!({"hooks": [{"type": "command", "command": "c"}]})
        );
    }

    #[test]
    fn an_agy_payload_parses_to_the_normalized_envelope() {
        let env = (AGY.parse)(agy_pre_tool_use());
        assert_eq!(env.kind, Kind::PreToolCall);
        assert_eq!(env.event, "PreToolUse");
        assert_eq!(
            env.session_id.as_deref(),
            Some("69348f19-f6b5-44b1-8eab-8c19e937f6d0")
        );
        assert_eq!(env.cwd.as_deref(), Some("/tmp/gaffqa"));
        assert_eq!(
            env.tool_name.as_deref(),
            Some("Bash"),
            "run_command is Bash"
        );
        assert_eq!(
            (AGY.tool_field)(&env.raw, "command").as_deref(),
            Some("echo hi")
        );
        assert_eq!((AGY.tool_field)(&env.raw, "file_path"), None);
        assert_eq!(env.raw["tool_use_id"], "step-2");
    }

    #[test]
    fn an_agy_tool_result_counts_once_by_its_step() {
        let env = (AGY.parse)(agy_post_tool_use());
        assert_eq!(env.kind, Kind::ToolCall);
        assert_eq!(env.tool_name.as_deref(), Some("Read"));
        assert_eq!(
            (AGY.tool_field)(&env.raw, "file_path").as_deref(),
            Some("/tmp/gaffqa/note.txt")
        );
        assert_eq!(env.raw["tool_use_id"], "step-6");
    }

    #[test]
    fn agy_maps_its_real_tool_names_onto_the_guard_names() {
        for (agy, guard) in [
            ("run_command", "Bash"),
            ("read_file", "Read"),
            ("write_file", "Write"),
            ("edit_file", "Edit"),
            ("grep_search", "grep_search"),
        ] {
            let mut p = agy_pre_tool_use();
            p["toolCall"]["name"] = json!(agy);
            assert_eq!((AGY.parse)(p).tool_name.as_deref(), Some(guard), "{agy}");
        }
    }

    #[test]
    fn agy_reads_the_first_model_call_as_a_prompt_and_later_ones_as_a_batch() {
        assert_eq!((AGY.parse)(agy_pre_invocation(0)).kind, Kind::Prompt);
        assert_eq!((AGY.parse)(agy_pre_invocation(1)).kind, Kind::ToolBatch);
        let mut bare = agy_pre_invocation(0);
        bare.as_object_mut().unwrap().remove("invocationNum");
        assert!(!(AGY.parse)(bare).kind.is_flush(), "no count, no flush");
    }

    #[test]
    fn an_agy_payload_without_an_event_name_is_never_a_guard_point() {
        // agy sends no event name. Without `--event`, the payload maps
        // to no kind, so it neither refuses nor flushes.
        let mut p = agy_pre_tool_use();
        p.as_object_mut().unwrap().remove("hookEventName");
        let env = (AGY.parse)(p);
        assert!(matches!(env.kind, Kind::Other(_)));
        assert_eq!(detect(None, &agy_pre_tool_use()).name, "agy");
    }

    #[test]
    fn agy_reads_context_as_an_injected_system_message() {
        // A real `agy -p` run quoted back a marker sent in this shape.
        assert_eq!(
            (AGY.context_output)("SessionStart", "hello"),
            r#"{"injectSteps":[{"systemMessage":{"systemMessage":"hello"}}]}"#
        );
    }

    #[test]
    fn agy_registers_the_event_on_the_command_in_its_own_shape() {
        assert_eq!(
            (AGY.registered_command)("PreInvocation", "gaff hook"),
            "gaff hook --host agy --event PreInvocation"
        );
        assert_eq!(
            (AGY.hook_entry)("PreToolUse", "c"),
            json!({"matcher": "*", "hooks": [{"type": "command", "command": "c"}]})
        );
        assert_eq!(
            (AGY.hook_entry)("SessionStart", "c"),
            json!({"type": "command", "command": "c"})
        );
        assert_eq!(AGY.hooks_key, "gaff");
        assert_eq!(AGY.failure_code, 0, "any non-zero exit refuses on agy");
    }
}
