//! The adapter seam: the one place where a host's shape is known.
//!
//! Everything above this module works on the normalized [`Envelope`].
//! An adapter owns the host-specific facts: how a raw hook payload maps
//! to an envelope, how injected context is rendered back to the host,
//! which events gaff subscribes to, and where `gaff init` writes the
//! registration.
//!
//! Output is a host fact too. A host reads injected context in its own
//! shape, so the rendering lives here, in `context_output`. Refusals and
//! holds do not: every host reads exit 2 as a refusal and the child's
//! stderr as the reason. That is a requirement on the host, not a shape
//! gaff renders. A host that blocks on some other code, or drops the
//! child's stderr, would break that contract.
//!
//! Two adapters ship. Claude Code is the host a person installs against.
//! The generic host speaks gaff's own normalized vocabulary, for a host
//! such as an agent runner that calls `gaff hook` itself; it reads only
//! gaff's field names, so it is not a guess at a real host's payload.
//! gaff does not guess a payload shape, because a guessed schema is worse
//! than an absent one: it fails at run time inside someone's session.
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
    /// The payload key that carries the event name. The parse reads it,
    /// and a test builds a sample payload with it, so a second adapter's
    /// vocabulary is not assumed to be this host's.
    pub event_key: &'static str,
    /// Whether `gaff init` registers hooks for this host. A host that
    /// calls `gaff hook` directly needs no settings file, so it does not
    /// self-register and carries an empty settings path.
    pub self_registers: bool,
    /// Map a raw hook payload to the normalized envelope.
    pub parse: fn(Value) -> Envelope,
    /// Render injected context to this host's stdout shape. Claude Code
    /// wraps it in its hook JSON; a generic host gets gaff's own
    /// normalized shape. This is the output a second host could not read
    /// while it was hardcoded in the CLI.
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
    /// A guard names a normalized tool and a field, not a payload
    /// shape, so the mapping from one to the other belongs to the
    /// adapter. It was read straight off `tool_input` in the CLI
    /// instead, which meant a host that nests tool input anywhere else
    /// would disarm every guard while `check` and `doctor` both
    /// reported them active.
    pub tool_field: fn(&Value, &str) -> Option<String>,
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
    // handler refuses. The feature shipped without this line, so gaff
    // was never called at a stop and every hold sat on disk unread.
    "Stop",
];

pub const CLAUDE_CODE: Adapter = Adapter {
    name: "claude-code",
    event_key: "hook_event_name",
    self_registers: true,
    parse: from_claude_code,
    context_output: claude_code_context_output,
    sniff: |json| json.get("hook_event_name").is_some(),
    settings_path: ".claude/settings.local.json",
    repo_settings_path: ".claude/settings.json",
    user_settings_path: ".claude/settings.json",
    session_env: "CLAUDE_CODE_SESSION_ID",
    hook_events: CLAUDE_CODE_EVENTS,
    tool_field: claude_code_tool_field,
};

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
/// does not self-register: a host such as kersh calls `gaff hook` itself
/// and needs no settings file. It reads exit 2 as a refusal and the
/// child's stderr as the reason, the same universal contract every host
/// uses; that is a requirement on the host, not a shape gaff renders.
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
};

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
/// A field may arrive as a string or as an argv array. Every element of
/// an array must be a string: dropping the ones that are not and
/// joining the rest invents a command line the host never sent, and the
/// invented one does not match, so the guard passes in silence.
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
pub const ADAPTERS: &[&Adapter] = &[&CLAUDE_CODE, &GENERIC];

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
        // Falling back is the safe direction, and doing it in silence
        // is not. A typo in GAFF_HOST reads as a working setting while
        // gaff parses somebody else's payload shape.
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
        // Stop is the last moment before the model walks away, and its
        // sink is verified. It is where every "drive the work to done"
        // rule actually applies.
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
    /// subscribes to. The stop hook shipped with `Stop` missing from
    /// the subscription list, so `gaff init` never registered it, the
    /// host never called gaff at a stop, and a hold sat on disk unread.
    /// The unit tests piped a Stop payload straight into the hook, so
    /// nothing noticed.
    #[test]
    fn every_flush_point_is_a_subscribed_event() {
        for adapter in ADAPTERS {
            let subscribed_kinds: Vec<crate::event::Kind> = adapter
                .hook_events
                .iter()
                .map(|event| {
                    let payload = serde_json::json!({
                        adapter.event_key: event,
                        "session_id": "s",
                    });
                    (adapter.parse)(payload).kind
                })
                .collect();
            for kind in [
                crate::event::Kind::SessionStart,
                crate::event::Kind::Prompt,
                crate::event::Kind::ToolBatch,
                crate::event::Kind::Stop,
            ] {
                assert!(kind.is_flush(), "{kind:?} is a flush point");
                assert!(
                    subscribed_kinds.contains(&kind),
                    "adapter `{}` flushes on {kind:?} but never subscribes to it, so that flush point is dead on this host",
                    adapter.name
                );
            }
        }
    }

    /// Guards gate on `Kind::PreToolCall`, and an unrecognized event
    /// falls through to `Kind::Other`. An adapter whose pre-tool event
    /// is named differently and never mapped gets zero guards, with
    /// nothing said. That is a whole feature off on a new host, so the
    /// mapping is asserted rather than assumed.
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

    /// A guard names a normalized tool and a field. Reading the payload
    /// in the CLI instead tied every guard to one host's schema, so a
    /// second adapter would have disarmed all of them while `check` and
    /// `doctor` reported them active.
    #[test]
    fn every_adapter_can_read_a_tool_field() {
        for adapter in ADAPTERS {
            let payload = serde_json::json!({
                "hook_event_name": adapter.hook_events[0],
                "session_id": "s",
                "tool_name": "Bash",
                "tool_input": {"command": "git status"},
            });
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
        for adapter in ADAPTERS {
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
            // line. Inventing one from the survivors passed the guard.
            let mixed = serde_json::json!({"tool_input": {"command": ["git", 7]}});
            assert_eq!((adapter.tool_field)(&mixed, "command"), None);
        }
    }
}
