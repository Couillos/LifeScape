#![forbid(unsafe_code)]
//! Hook behind `logln!`: every daemon log line is offered to one
//! registered sink (the errors collector, which also feeds the idle
//! detector's reasons). The core cannot name the daemon, so the binary
//! registers the sink at startup, before any thread starts (layering step
//! A6, 2026-11-23). Without a registration the lines are only printed:
//! `assert_registered` makes that a loud failure at startup instead of a
//! silent loss of the idle reasons.

use std::sync::OnceLock;

static SINK: OnceLock<fn(&str)> = OnceLock::new();

/// Register the sink; the first registration wins, later ones are no-ops.
pub fn register(sink: fn(&str)) {
    let _ = SINK.set(sink);
}

pub fn is_registered() -> bool {
    SINK.get().is_some()
}

/// Panics when no sink is registered (called by `main` once registered).
pub fn assert_registered() {
    assert!(is_registered(), "log sink not registered: brain idle reasons and the errors collector would silently stop");
}

/// Offer one log line to the sink (a no-op while none is registered).
pub fn emit(line: &str) {
    if let Some(sink) = SINK.get() {
        sink(line);
    }
}
