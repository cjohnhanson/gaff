//! The short name, so `uvx gaffr` and `npx gaffr` work.
//!
//! maturin ties an installed command name to the Cargo bin name, and
//! refuses a `[project.scripts]` entry beside a binary. A wheel
//! published as `gaffr` therefore needs a `gaffr` command. Without one,
//! `uvx gaffr` errors and tells the reader to type
//! `uvx --from gaffr gaff`.
//!
//! This execs `gaff` beside it rather than carrying a second copy, so
//! both names work from one install.
use std::os::unix::process::CommandExt;

fn main() -> std::process::ExitCode {
    let Ok(me) = std::env::current_exe() else {
        eprintln!("gaffr: cannot resolve its own path");
        return std::process::ExitCode::FAILURE;
    };
    let real = me.with_file_name("gaff");
    let err = std::process::Command::new(&real)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("gaffr: cannot run {}: {err}", real.display());
    std::process::ExitCode::FAILURE
}
