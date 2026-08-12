//! Event envelope and per-event capability table.
//!
//! The envelope is the stable contract handlers see: a small versioned core
//! plus the verbatim upstream payload under `raw` (unstable, adapter-owned).
//!
//! The capability table encodes what each hook event actually supports —
//! whether it can block, where injected context lands, and any upstream
//! timeout ceiling. Entries are added only once verified against the live
//! hook reference; unknown events get `Capability::UNVERIFIED`, which
//! permits nothing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Schema version stamped on every envelope this binary emits.
pub const SCHEMA_VERSION: u32 = 1;

/// Where a hook event's injected output lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSink {
    /// No context channel: output is discarded by the harness.
    None,
    /// Context is delivered to the model as session/system framing.
    /// The only sink safe for prime sections and reminders.
    AgentContext,
    /// Context is attached to a tool result and is indistinguishable from
    /// tool output to the model. Never decorate (the qei8 bug class).
    ToolResult,
    /// Stdout *replaces* the event's payload (e.g. prompt expansion).
    ReplacesPayload,
    /// Stdout is structured data the harness consumes (e.g. a worktree
    /// path). Any injected byte is corruption.
    StdoutIsData,
}

/// What one hook event supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub can_block: bool,
    pub context_sink: ContextSink,
    /// Upstream timeout ceiling in milliseconds, where the reference
    /// documents one. Configured handler timeouts must clamp to this.
    pub timeout_ceiling_ms: Option<u64>,
    /// Whether this entry was verified against the live hook reference.
    /// Unverified entries permit nothing.
    pub verified: bool,
}

impl Capability {
    /// The conservative default for events not (yet) in the table:
    /// cannot block, no context sink, unverified.
    pub const UNVERIFIED: Self = Self {
        can_block: false,
        context_sink: ContextSink::None,
        timeout_ceiling_ms: None,
        verified: false,
    };

    const fn verified(can_block: bool, sink: ContextSink, ceiling_ms: Option<u64>) -> Self {
        Self {
            can_block,
            context_sink: sink,
            timeout_ceiling_ms: ceiling_ms,
            verified: true,
        }
    }

    /// True when injecting context into this event is safe: the sink is
    /// the model's session framing, not a tool result or a data channel.
    #[must_use]
    pub fn injection_safe(&self) -> bool {
        self.verified && self.context_sink == ContextSink::AgentContext
    }
}

/// Capability lookup for a Claude Code hook event name.
///
/// Sources: entries reflect the hook reference as verified during design
/// review (2026-08). `PostToolUse` context rides the tool result;
/// `PostToolBatch` fires after a parallel batch resolves, before the next
/// model call, and is attached to no tool result; `WorktreeCreate` stdout
/// is the worktree path; `UserPromptExpansion` stdout replaces the
/// expansion; `Notification`/`StopFailure`/`InstructionsLoaded`/
/// `DirectoryAdded` discard output; `SessionEnd` shares a 1.5s budget.
#[must_use]
pub fn capability(event_name: &str) -> Capability {
    match event_name {
        "SessionStart" | "PostToolBatch" => {
            Capability::verified(false, ContextSink::AgentContext, None)
        }
        "UserPromptSubmit" => Capability::verified(true, ContextSink::AgentContext, Some(30_000)),
        "PreToolUse" | "Stop" => Capability::verified(true, ContextSink::AgentContext, None),
        "PostToolUse" | "PostToolUseFailure" => {
            Capability::verified(false, ContextSink::ToolResult, None)
        }
        "UserPromptExpansion" => Capability::verified(false, ContextSink::ReplacesPayload, None),
        "WorktreeCreate" => Capability::verified(false, ContextSink::StdoutIsData, None),
        "Notification" | "StopFailure" | "InstructionsLoaded" | "DirectoryAdded" => {
            Capability::verified(false, ContextSink::None, None)
        }
        "MessageDisplay" => Capability::verified(false, ContextSink::None, Some(10_000)),
        "SessionEnd" => Capability::verified(false, ContextSink::None, Some(1_500)),
        _ => Capability::UNVERIFIED,
    }
}

/// The stable core every handler payload carries. Everything else lives
/// under `raw`, verbatim from upstream, documented as unstable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub gaff_schema: u32,
    /// Upstream event name, forwarded verbatim — unknown events are
    /// first-class, never dropped.
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Verbatim upstream payload. Unstable; adapter-owned.
    pub raw: Value,
}

impl Envelope {
    /// Build an envelope from a Claude Code hook payload.
    #[must_use]
    pub fn from_claude_code(json: Value) -> Self {
        let get = |key: &str| {
            json.get(key)
                .and_then(Value::as_str)
                .map(std::string::ToString::to_string)
        };
        Self {
            gaff_schema: SCHEMA_VERSION,
            event: get("hook_event_name").unwrap_or_else(|| "Unknown".to_string()),
            session_id: get("session_id"),
            cwd: get("cwd"),
            tool_name: get("tool_name"),
            raw: json,
        }
    }

    /// Capability profile for this envelope's event.
    #[must_use]
    pub fn capability(&self) -> Capability {
        capability(&self.event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn post_tool_use_is_never_injection_safe() {
        let cap = capability("PostToolUse");
        assert!(
            !cap.injection_safe(),
            "PostToolUse context rides the tool result"
        );
        assert_eq!(cap.context_sink, ContextSink::ToolResult);
        assert!(!cap.can_block, "the tool already ran");
    }

    #[test]
    fn post_tool_batch_is_the_injection_home() {
        let cap = capability("PostToolBatch");
        assert!(cap.injection_safe());
        assert!(!cap.can_block);
    }

    #[test]
    fn session_start_and_prompt_submit_are_injection_safe() {
        assert!(capability("SessionStart").injection_safe());
        assert!(capability("UserPromptSubmit").injection_safe());
    }

    #[test]
    fn data_channel_events_reject_injection() {
        for name in ["WorktreeCreate", "UserPromptExpansion"] {
            let cap = capability(name);
            assert!(
                !cap.injection_safe(),
                "{name} stdout is not a context channel"
            );
        }
    }

    #[test]
    fn discarded_output_events_reject_injection() {
        for name in [
            "Notification",
            "StopFailure",
            "InstructionsLoaded",
            "DirectoryAdded",
        ] {
            assert_eq!(capability(name).context_sink, ContextSink::None, "{name}");
        }
    }

    #[test]
    fn timeout_ceilings_from_reference() {
        assert_eq!(capability("SessionEnd").timeout_ceiling_ms, Some(1_500));
        assert_eq!(
            capability("MessageDisplay").timeout_ceiling_ms,
            Some(10_000)
        );
        assert_eq!(
            capability("UserPromptSubmit").timeout_ceiling_ms,
            Some(30_000)
        );
    }

    #[test]
    fn unknown_events_permit_nothing_but_are_preserved() {
        let cap = capability("SomeFutureEvent");
        assert!(!cap.verified);
        assert!(!cap.can_block);
        assert!(!cap.injection_safe());

        let env = Envelope::from_claude_code(json!({
            "hook_event_name": "SomeFutureEvent",
            "session_id": "s-1",
            "novel_field": {"x": 1},
        }));
        assert_eq!(env.event, "SomeFutureEvent");
        assert_eq!(
            env.raw["novel_field"]["x"], 1,
            "raw payload forwarded verbatim"
        );
    }

    #[test]
    fn envelope_extracts_stable_core() {
        let env = Envelope::from_claude_code(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "abc-123",
            "cwd": "/some/repo",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
        }));
        assert_eq!(env.gaff_schema, SCHEMA_VERSION);
        assert_eq!(env.event, "PreToolUse");
        assert_eq!(env.session_id.as_deref(), Some("abc-123"));
        assert_eq!(env.cwd.as_deref(), Some("/some/repo"));
        assert_eq!(env.tool_name.as_deref(), Some("Bash"));
        assert!(env.capability().can_block);
    }

    #[test]
    fn missing_event_name_maps_to_unknown() {
        let env = Envelope::from_claude_code(json!({"session_id": "s"}));
        assert_eq!(env.event, "Unknown");
        assert!(!env.capability().verified);
    }
}
