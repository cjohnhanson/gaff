//! gaff — a context-lifecycle handler for coding agents.
//!
//! This binary is a shim. The command line surface lives in
//! [`gaff::cli`], so it can be tested in process.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    gaff::cli::run(&args)
}
