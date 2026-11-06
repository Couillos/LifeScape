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
    /// The planner found no route: carries the diagnosis (reason, closest
    /// reachable tile, missing requirements, nearby disabled transitions).
    Travel(String),
    /// A walk leg made no progress for the stall bound: carries the leg,
    /// the suspect tile and the last game message.
    StepRefused(String),
    /// A planned door never opened (or its crossing step failed): carries
    /// the door, the position and the last game message.
    DoorDidNotOpen(String),
    /// A transition ride never moved the bot: carries the transition,
    /// the stand and the last game message.
    RideDidNotMove(String),
    /// A transition ride landed somewhere unexpected: carries expected
    /// versus observed position.
    RideWrongDestination(String),
    /// A choice dialogue opened mid-ride: carries the prompt and options.
    /// The executor never picks; the caller (or prober data) decides.
    NeedsDialogueChoice(String),
    /// Final navigation failure with its replay dossier (§6.4): the kind,
    /// the failing segment, where the bot stood, what was wanted versus
    /// seen, and the dossier path when one was written.
    TravelFailure(TravelError),
}

/// Final navigation failure kind (§6.4): one per executor outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TravelErrorKind {
    NoRoute,
    StepRefused,
    DoorDidNotOpen,
    RideDidNotMove,
    RideWrongDestination,
    NeedsDialogueChoice,
    Timeout,
    Interrupted,
    Io,
}

impl TravelErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TravelErrorKind::NoRoute => "NoRoute",
            TravelErrorKind::StepRefused => "StepRefused",
            TravelErrorKind::DoorDidNotOpen => "DoorDidNotOpen",
            TravelErrorKind::RideDidNotMove => "RideDidNotMove",
            TravelErrorKind::RideWrongDestination => "RideWrongDestination",
            TravelErrorKind::NeedsDialogueChoice => "NeedsDialogueChoice",
            TravelErrorKind::Timeout => "Timeout",
            TravelErrorKind::Interrupted => "Interrupted",
            TravelErrorKind::Io => "Io",
        }
    }

    /// Executor classification of a terminal error. Plan-side failures
    /// (custom edge with no live executor, unreachable, state) file under
    /// NoRoute: the plan asked for something unexecutable.
    pub fn of(error: &Error) -> TravelErrorKind {
        match error {
            Error::StepRefused(_) => TravelErrorKind::StepRefused,
            Error::DoorDidNotOpen(_) => TravelErrorKind::DoorDidNotOpen,
            Error::RideDidNotMove(_) => TravelErrorKind::RideDidNotMove,
            Error::RideWrongDestination(_) => TravelErrorKind::RideWrongDestination,
            Error::NeedsDialogueChoice(_) => TravelErrorKind::NeedsDialogueChoice,
            Error::Timeout(_) => TravelErrorKind::Timeout,
            Error::Interrupted(_) => TravelErrorKind::Interrupted,
            Error::Io(_) | Error::Login(_) => TravelErrorKind::Io,
            _ => TravelErrorKind::NoRoute,
        }
    }
}

/// One failed `travel_to`: what broke, where, and the replay dossier.
#[derive(Debug, Clone)]
pub struct TravelError {
    pub kind: TravelErrorKind,
    pub segment_index: Option<usize>,
    /// (x, z, level): explicit order, unlike `World::position`.
    pub position: (i32, i32, i32),
    pub tick: u64,
    pub expected: String,
    pub observed: String,
    pub dossier: Option<std::path::PathBuf>,
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
            Error::Travel(detail) => write!(formatter, "travel: {detail}"),
            Error::StepRefused(detail) => write!(formatter, "step refused: {detail}"),
            Error::DoorDidNotOpen(detail) => write!(formatter, "door did not open: {detail}"),
            Error::RideDidNotMove(detail) => write!(formatter, "ride did not move: {detail}"),
            Error::RideWrongDestination(detail) => write!(formatter, "ride wrong destination: {detail}"),
            Error::NeedsDialogueChoice(detail) => write!(formatter, "needs dialogue choice: {detail}"),
            Error::TravelFailure(failure) => write!(
                formatter,
                "travel failure {} seg={:?} at {:?} t={} expected={} observed={} dossier={:?}",
                failure.kind.as_str(),
                failure.segment_index,
                failure.position,
                failure.tick,
                failure.expected,
                failure.observed,
                failure.dossier,
            ),
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
