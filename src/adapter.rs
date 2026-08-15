//! The adapter seam: the one place where a host's shape is known.
//!
//! Everything above this module works on the normalized [`Envelope`].
//! An adapter owns three host-specific facts: how a raw hook payload
//! maps to an envelope, which events gaff subscribes to, and where
//! `gaff init` writes the registration.
//!
//! Claude Code is the only implemented adapter. That is a statement of
//! what is built, not of what the design allows. gaff does not guess
//! another host's payload shape, because a guessed schema is worse than
//! an absent one: it fails at run time inside someone's session.
//!
//! # Adding an adapter
//!
//! Add a [`Adapter`] constant with the host's real field names, its
//! real event names, and its real settings path, taken from that host's
//! documentation. Add it to [`ADAPTERS`]. Give `sniff` a predicate that
//! matches only that host's payload. Nothing else in gaff changes.

use serde_json::Value;

use crate::event::{Envelope, Kind, SCHEMA_VERSION};

/// One agent host.
pub struct Adapter {
    /// The name for `--host` and `GAFF_HOST`.
    pub name: &'static str,
    /// Map a raw hook payload to the normalized envelope.
    pub parse: fn(Value) -> Envelope,
    /// Recognize this host's payload by its shape. Detection prefers an
    /// explicit name; this is the fallback.
    pub sniff: fn(&Value) -> bool,
    /// Where `gaff init` registers the hooks, relative to the repo root.
    pub settings_path: &'static str,
    /// The events gaff subscribes to on this host.
    pub hook_events: &'static [&'static str],
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
];

pub const CLAUDE_CODE: Adapter = Adapter {
    name: "claude-code",
    parse: from_claude_code,
    sniff: |json| json.get("hook_event_name").is_some(),
    settings_path: ".claude/settings.local.json",
    hook_events: CLAUDE_CODE_EVENTS,
};

/// Every implemented adapter.
pub const ADAPTERS: &[&Adapter] = &[&CLAUDE_CODE];

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
    if let Some(name) = explicit
        && let Some(found) = by_name(name)
    {
        return found;
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
    fn every_adapter_declares_a_settings_path_and_events() {
        for adapter in ADAPTERS {
            assert!(!adapter.settings_path.is_empty(), "{}", adapter.name);
            assert!(!adapter.hook_events.is_empty(), "{}", adapter.name);
            assert!(by_name(adapter.name).is_some(), "{}", adapter.name);
        }
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
        assert!(!env.kind.is_flush(), "an unknown event is never a flush point");
        assert!(!env.capability().verified);
    }

    #[test]
    fn the_flush_points_are_the_cross_agent_set() {
        assert!(Kind::SessionStart.is_flush());
        assert!(Kind::Prompt.is_flush());
        assert!(Kind::ToolBatch.is_flush());
        // A tool call's context rides the tool result, never the framing.
        assert!(!Kind::ToolCall.is_flush());
        assert!(!Kind::Stop.is_flush());
    }

    #[test]
    fn a_normalized_name_round_trips_through_a_config() {
        for k in [Kind::SessionStart, Kind::Prompt, Kind::ToolCall, Kind::ToolBatch, Kind::Stop] {
            assert_eq!(Kind::parse(k.as_str()), k);
        }
    }
}
