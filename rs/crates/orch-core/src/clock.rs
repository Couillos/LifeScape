#![forbid(unsafe_code)]
//! Wall clock shared by every layer (hoisted out of `jobs` so the core
//! modules do not depend on the fleet: layering step A1, 2026-11-23).

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix seconds as a float (`jobs::now_secs` re-exports it).
pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_secs_f64())
        .unwrap_or(0.0)
}
