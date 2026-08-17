//! gaff — a context-lifecycle handler for coding agents.
//!
//! gaff counts the hook events of a session. It re-injects context on a
//! cadence. It delivers prime sections and advisory profiles.
//!
//! gaff registers as one handler in the harness's own hook config. It
//! owns no dispatch. It blocks nothing. It injects context only on the
//! events whose context sink is the model's session framing.

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
pub mod init;
pub mod reviewnote;
pub mod state;
