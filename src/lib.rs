//! gaff — context-lifecycle handler for coding agents.
//!
//! Counters over the hook event stream, cadence-driven context
//! re-injection, prime sections, and advisory profiles. Registers as a
//! handler in the agent harness's native hook config; owns no dispatch,
//! blocks nothing, and injects only on events whose context sink is the
//! model's session framing.

pub mod config;
pub mod docs;
pub mod engine;
pub mod event;
pub mod init;
pub mod state;
