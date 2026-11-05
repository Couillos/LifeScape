//! Typed SDK errors with position/target/state context.
//!
//! Every failure carries where the bot stood, what it wanted, and what it
//! saw, so a script (or a log reader) never has to guess. No silent
//! fallbacks: errors propagate to the caller.

/// Typed failure for every SDK operation.
#[derive(Debug, Clone)]
pub enum Error {
    /// Socket or file failure: what was attempted and where.
    Io(String),
    /// Login refused by the server (bad credentials, stale session...).
    Login(String),
    /// Unknown opcode or malformed frame: includes the raw bytes seen.
    Protocol(String),
    /// No reachable tile, door, target or portal for the request.
    Unreachable(String),
    /// The SDK cannot read or expose player state (bad table, bad write).
    State(String),
    /// A tick-bounded wait ran out: includes position, target and ticks.
    Timeout(String),
    /// Operator asked the run to stop (unload/close): unwinds the script
    /// to the next pump instead of finishing the current action.
    Interrupted(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(detail) => write!(formatter, "io: {detail}"),
            Error::Login(detail) => write!(formatter, "login: {detail}"),
            Error::Protocol(detail) => write!(formatter, "protocol: {detail}"),
            Error::Unreachable(detail) => write!(formatter, "unreachable: {detail}"),
            Error::State(detail) => write!(formatter, "state: {detail}"),
            Error::Timeout(detail) => write!(formatter, "timeout: {detail}"),
            Error::Interrupted(detail) => write!(formatter, "interrupted: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Io(err.to_string())
    }
}

impl From<crate::buffer::ReadError> for Error {
    fn from(err: crate::buffer::ReadError) -> Error {
        Error::Protocol(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// True for a cooperative stop: retry loops must propagate it instead of
/// converting it into Unreachable/Timeout and walking on.
pub fn is_interrupted(error: &Error) -> bool {
    matches!(error, Error::Interrupted(_))
}
