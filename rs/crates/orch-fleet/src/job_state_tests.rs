//! Round trip text <-> enumeration (lot 5.1): every word the worker can
//! send and every word seen in the live store and daemon log.

use super::{Phase, Status};

/// Every word of the store's `error_events` (`stop`, `kind`) and of the
/// daemon log (`restored <account> (<status>)`, `task_report stop=`,
/// `busy: <x> is <status>`), read 2026-11-24 (read-only: `sqlite3
/// "file:run/lifescape.db?mode=ro"`, `run/orchestrator.log`).
const SEEN_IN_STORE_AND_LOG: &[&str] = &[
    "abandoned", "blocked", "Blocked", "budget_exhausted", "daemon_restart", "DoorDidNotOpen", "failed", "Failed", "first", "goal_reached", "goal_stuck", "graph_stuck", "idle", "idle_bot", "Io", "locked", "mismatch",
    "NeedsDialogueChoice", "no_progress", "NoRoute", "no_show", "RideDidNotMove", "RideWrongDestination", "running", "Sdk", "short", "skill_banned", "StepRefused", "stopped", "stranded", "timeout", "Timeout", "Usage",
];

/// The words the code writes into a job (`daemon`, `supervise`, `session`).
const STATUS_WORDS: &[&str] = &["starting", "idle", "running", "busy", "finished", "failed", "blocked", "stopped", "reconnecting", "disconnected"];
const PHASE_WORDS: &[&str] = &["idle", "running", "failed"];

#[test]
fn every_known_word_is_a_variant_and_round_trips() {
    for word in STATUS_WORDS {
        let status = Status::parse(word);
        assert!(!matches!(status, Status::Other(_)), "{word} is a variant");
        assert_eq!(status.as_str(), *word);
        assert_eq!(status.to_string(), *word);
    }
    for word in PHASE_WORDS {
        let phase = Phase::parse(word);
        assert!(!matches!(phase, Phase::Other(_)), "{word} is a variant");
        assert_eq!(phase.as_str(), *word);
        assert_eq!(phase.to_string(), *word);
    }
}

#[test]
fn every_word_seen_in_the_store_and_the_log_survives_the_round_trip() {
    for word in SEEN_IN_STORE_AND_LOG.iter().chain(STATUS_WORDS).chain(PHASE_WORDS).chain(&["", "paused", "CAPS"]) {
        assert_eq!(Status::parse(word).as_str(), *word, "status {word:?}");
        assert_eq!(Phase::parse(word).as_str(), *word, "phase {word:?}");
        assert_eq!(Status::parse(Status::parse(word).as_str()), Status::parse(word));
    }
    // The statuses the daemon log shows are variants, not `Other`.
    for word in ["idle", "running", "failed", "blocked", "stopped"] {
        assert!(!matches!(Status::parse(word), Status::Other(_)), "{word}");
    }
    // An unknown word is kept, never mapped to a known one.
    assert_eq!(Status::parse("paused"), Status::Other("paused".to_string()));
    assert_ne!(Phase::parse("Idle"), Phase::Idle, "the wire text is case sensitive, as it was");
}
