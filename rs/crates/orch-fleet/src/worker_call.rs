#![forbid(unsafe_code)]
//! What can go wrong in one call to a session worker, typed (lot 5.3 of
//! the architecture plan): the daemon used to tell them apart by the
//! prefix or a word of an error text (`"busy: ..."`, `"refused"`,
//! `"no socket"`). The text stays what it was (logs, the `error` key of
//! a reply, the dashboard): `text()` / `Display` give it back unchanged.

use std::fmt;

/// A worker that answered `ok: false`, by what it said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `busy: ...`: a script runs, an unload is under way, the session
    /// reconnects. The run never started.
    Busy(String),
    /// No session took the call (`no session, open X first`, `worker for
    /// X died, open X first`). The run never started.
    NoSession(String),
    /// Any other refusal (a bad script name, a refused command).
    Other(String),
}

impl Refusal {
    /// Reads the `error` text of a refusing reply.
    pub fn parse(error: &str) -> Refusal {
        if error.starts_with("busy") {
            Refusal::Busy(error.to_string())
        } else if error.starts_with("no session") || error.contains(" died, open ") {
            Refusal::NoSession(error.to_string())
        } else {
            Refusal::Other(error.to_string())
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Refusal::Busy(text) | Refusal::NoSession(text) | Refusal::Other(text) => text,
        }
    }

    /// True when the refusal proves the script never started (P2-5: the
    /// node goes back to ready, uncounted). A transport error is no
    /// proof: crash recovery keeps judging it.
    pub fn never_ran(&self) -> bool {
        matches!(self, Refusal::Busy(_) | Refusal::NoSession(_))
    }
}

/// One failed worker call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// No socket, a refused connect, a worker that died (`Injoignable`).
    Unreachable(String),
    /// The connect or the answer took longer than the bound (`Délai`).
    TimedOut(String),
    /// The wire broke: a write, a read, a line that is not JSON (`Protocole`).
    Protocol(String),
    /// The worker answered and said no (`Refus`).
    Refused(Refusal),
}

impl CallError {
    pub fn text(&self) -> &str {
        match self {
            CallError::Unreachable(text) | CallError::TimedOut(text) | CallError::Protocol(text) => text,
            CallError::Refused(refusal) => refusal.text(),
        }
    }

    /// True for an unreachable worker (its socket is stale).
    pub fn is_unreachable(&self) -> bool {
        matches!(self, CallError::Unreachable(_))
    }

    /// An unreachable worker whose socket nobody listens on any more
    /// (refused connect, vanished file): safe to remove. A slow or busy
    /// worker keeps its socket (removing it orphaned the worker, still
    /// logged in, unreachable).
    pub fn is_socket_stale(&self) -> bool {
        self.is_unreachable() && ["refused", "No such file", "not found"].iter().any(|word| self.text().contains(word))
    }

    /// A refusal that proves the script never started.
    pub fn never_ran(&self) -> bool {
        matches!(self, CallError::Refused(refusal) if refusal.never_ran())
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.text())
    }
}

impl From<CallError> for String {
    fn from(error: CallError) -> String {
        error.text().to_string()
    }
}

#[cfg(test)]
#[path = "worker_call_tests.rs"]
mod tests;
