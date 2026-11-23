#![forbid(unsafe_code)]
//! Text markers shared between the modules that write a log line or a
//! blocked-need tag and the errors model that classifies it. They live
//! in the core so the stats layer does not import the brain (layering
//! step A1, 2026-11-23); the brain modules re-export them.

/// Tag a script blocks with on a contended site (`kit::site::CONTENDED_TAG`).
pub const CONTENDED_TAG: &str = "contended:";
/// Tag a script blocks with on a depleted site (`kit::deplete::DEPLETED_TAG`, user 2026-11-20).
pub const DEPLETED_TAG: &str = "depleted:";
/// Tag of a locked need (`kit::locked::LOCKED_TAG`, plan P5).
pub const LOCKED_TAG: &str = "locked:";
/// Tail of the `brain <account>: idle <n>s without a script: ...` line.
pub const ALERT_MARKER: &str = "s without a script: ";
/// Head of the `brain <account>: stranded at (x,z,l)` line.
pub const STRANDED_MARKER: &str = "stranded at ";
