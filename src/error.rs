//! The error type.
//!
//! Every variant here is a message a person reads, never a control
//! signal. gaff answers every failure the same way: it warns and
//! continues. It never exits 2, because the agent side treats exit 2
//! as the blocking code and no gaff failure may block a session.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read the hook payload from stdin")]
    UnreadableStdin,

    #[error("the hook payload is not valid JSON")]
    InvalidPayload,

    #[error("no state directory. Set GAFF_STATE_DIR or HOME.")]
    NoStateDir,

    #[error("cannot resolve the working directory")]
    NoWorkingDir,

    #[error("no session. Pass --session or set CLAUDE_CODE_SESSION_ID.")]
    NoSession,

    #[error("the config {path} is not valid: {detail}")]
    BrokenConfig { path: String, detail: String },

    #[error("unknown profile `{0}`")]
    UnknownProfile(String),

    #[error(
        "profile `{0}` is human-only. Add it to transitions.agent_may_set to allow an agent switch."
    )]
    HumanOnlyProfile(String),

    #[error("unknown host `{name}` (implemented: {known})")]
    UnknownHost { name: String, known: String },

    #[error(
        "this repo is not trusted, so no handler ran. Run `gaff trust` from a terminal to allow it."
    )]
    UntrustedRepo,

    #[error(
        "gaff trust must be run from a terminal. An agent may not grant itself the right to run commands."
    )]
    TrustNeedsTerminal,

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_reads_as_a_sentence_to_a_person() {
        // A message is user-facing prose, so it must not be empty and
        // must not leak a Rust type name.
        let samples = [
            Error::UnreadableStdin,
            Error::InvalidPayload,
            Error::NoStateDir,
            Error::NoWorkingDir,
            Error::NoSession,
            Error::UntrustedRepo,
            Error::TrustNeedsTerminal,
            Error::UnknownProfile("x".into()),
            Error::HumanOnlyProfile("x".into()),
        ];
        for e in samples {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "{e:?}");
            assert!(!msg.contains("Error::"), "{msg}");
        }
    }

    #[test]
    fn an_io_error_carries_its_own_message() {
        let e: Error = std::io::Error::other("disk on fire").into();
        assert_eq!(e.to_string(), "disk on fire");
    }
}
