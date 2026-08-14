//! The event envelope and the per-event capability table.
//!
//! The envelope is the stable contract that a handler sees. It holds a
//! small versioned core and the verbatim upstream payload under `raw`.
//! The `raw` payload is unstable, and the adapter owns it.
//!
//! The capability table records what each hook event supports: whether
//! the event can block, where the injected context lands, and the
//! upstream timeout ceiling. Add an entry only after you verify it
//! against the live hook reference. An unknown event gets
//! `Capability::UNVERIFIED`, which permits nothing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The schema version stamped on every envelope this binary emits.
pub const SCHEMA_VERSION: u32 = 1;

/// Where a hook event's injected output lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSink {
    /// No context channel. The harness discards the output.
    None,
    /// The harness delivers the context to the model as session framing.
    /// This is the only sink that is safe for a section or a reminder.
    AgentContext,
    /// The harness attaches the context to a tool result. The model reads
    /// it as tool output. Never inject here (the qei8 bug class).
    ToolResult,
    /// Stdout replaces the payload of the event. Prompt expansion uses
    /// this sink.
    ReplacesPayload,
    /// Stdout is structured data that the harness reads, such as a
    /// worktree path. An injected byte corrupts that data.
    StdoutIsData,
}

/// What one hook event supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub can_block: bool,
    pub context_sink: ContextSink,
    /// The upstream timeout ceiling in milliseconds, where the reference
    /// documents one. Clamp a configured handler timeout to this value.
    pub timeout_ceiling_ms: Option<u64>,
    /// Whether this entry was verified against the live hook reference.
    /// An unverified entry permits nothing.
    pub verified: bool,
}

impl Capability {
    /// The safe default for an event that is not yet in the table. It
    /// cannot block, it has no context sink, and it is unverified.
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

    /// True when it is safe to inject context into this event. The sink
    /// must be the model's session framing. A tool result and a data
    /// channel are not safe.
    #[must_use]
    pub fn injection_safe(&self) -> bool {
        self.verified && self.context_sink == ContextSink::AgentContext
    }
}

/// Look up the capability of a Claude Code hook event name.
///
/// The entries follow the hook reference, as verified during the design
/// review of 2026-08:
///
/// - `PostToolUse` context rides the tool result.
/// - `PostToolBatch` fires after a parallel batch resolves, before the
///   next model call. It attaches to no tool result.
/// - `WorktreeCreate` stdout is the worktree path.
/// - `UserPromptExpansion` stdout replaces the expansion.
/// - `Notification`, `StopFailure`, `InstructionsLoaded`, and
///   `DirectoryAdded` discard the output.
/// - `SessionEnd` shares a budget of 1.5 seconds.
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

/// The stable core that every handler payload carries. Everything else
/// lives under `raw`. The `raw` value comes verbatim from upstream, and
/// it is unstable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub gaff_schema: u32,
    /// The upstream event name, forwarded verbatim. An unknown event is
    /// first-class, and gaff never drops it.
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// The verbatim upstream payload. It is unstable, and the adapter
    /// owns it.
    pub raw: Value,
}

impl Envelope {
    /// Build an envelope from a Claude Code hook payload.
    ///
    /// The host-specific mapping lives in [`crate::adapter`]. This
    /// delegate keeps the call sites in the tests short.
    #[must_use]
    pub fn from_claude_code(json: Value) -> Self {
        (crate::adapter::CLAUDE_CODE.parse)(json)
    }

    /// The capability of this envelope's event.
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
            "gaff forwards the raw payload verbatim"
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
