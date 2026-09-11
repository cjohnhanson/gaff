//! gaff — a context-lifecycle handler for coding agents.
//!
//! gaff counts the hook events of a session and re-injects context on a
//! cadence. It delivers prime sections and advisory profiles, refuses a
//! tool call that matches a guard, holds a stop, dispatches the declared
//! git hooks, and renders GitHub workflows from the same config.
//!
//! gaff registers as one handler in the harness's own hook config. It
//! owns no dispatch there. It injects context only on the events whose
//! context sink is the model's session framing.

pub mod adapter;
pub mod cli;
pub mod config;
pub mod docs;
pub mod engine;
pub mod error;
pub mod event;
pub mod ghworkflow;
pub mod githook;
pub mod guard;
pub mod handler;
pub mod hookagent;
pub mod init;
pub mod reviewnote;
pub mod state;
