#![forbid(unsafe_code)]
//! The session state of a job as the daemon holds it (lot 5.1 of the
//! architecture plan): `Phase` and `Status` are enumerations in memory,
//! the text stays on the wire (worker replies, heartbeat frames, the
//! dashboard, the state files). One codec per type: `parse` never fails
//! (an unknown word, from a newer worker, is kept as `Other` and written
//! back unchanged), `as_str` gives the wire text.

use std::fmt;

/// What the worker's script slot is doing: `phase` of a `status` reply
/// or a heartbeat frame.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Idle,
    Running,
    Failed,
    /// A word this build does not know, kept as received.
    Other(String),
}

impl Phase {
    pub fn parse(text: &str) -> Phase {
        match text {
            "idle" => Phase::Idle,
            "running" => Phase::Running,
            "failed" => Phase::Failed,
            other => Phase::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Phase::Idle => "idle",
            Phase::Running => "running",
            Phase::Failed => "failed",
            Phase::Other(text) => text,
        }
    }
}

/// No word yet (nothing received): `Other` of the empty text, which the
/// wire already read as "nothing".
impl Default for Phase {
    fn default() -> Self {
        Phase::Other(String::new())
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.as_str())
    }
}

/// The worker session's status: `status` of a `status` reply or a
/// heartbeat frame (`busy` is the worker's own, held while a command runs).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Starting,
    Idle,
    Running,
    Busy,
    Finished,
    Failed,
    Blocked,
    Stopped,
    Reconnecting,
    Disconnected,
    /// A word this build does not know, kept as received.
    Other(String),
}

impl Status {
    pub fn parse(text: &str) -> Status {
        match text {
            "starting" => Status::Starting,
            "idle" => Status::Idle,
            "running" => Status::Running,
            "busy" => Status::Busy,
            "finished" => Status::Finished,
            "failed" => Status::Failed,
            "blocked" => Status::Blocked,
            "stopped" => Status::Stopped,
            "reconnecting" => Status::Reconnecting,
            "disconnected" => Status::Disconnected,
            other => Status::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Status::Starting => "starting",
            Status::Idle => "idle",
            Status::Running => "running",
            Status::Busy => "busy",
            Status::Finished => "finished",
            Status::Failed => "failed",
            Status::Blocked => "blocked",
            Status::Stopped => "stopped",
            Status::Reconnecting => "reconnecting",
            Status::Disconnected => "disconnected",
            Status::Other(text) => text,
        }
    }
}

/// No word yet (nothing received): `Other` of the empty text.
impl Default for Status {
    fn default() -> Self {
        Status::Other(String::new())
    }
}

impl fmt::Display for Status {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "job_state_tests.rs"]
mod tests;
