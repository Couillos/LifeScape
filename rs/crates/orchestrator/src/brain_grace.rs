#![forbid(unsafe_code)]
//! Uncounted retries (split from `brain.rs`, rule 4): an active node
//! whose worker reads idle without a report after a daemon restart or an
//! operator `unload`/`close`/`open` (redeploy) is a stop, never a crash.
//! A stop flag covers the run it interrupted only: the next launch clears
//! it (`brain::note_launch`), so a later crash still counts.

/// True the first time this daemon finds `account`'s active node with an
/// idle worker and no report: after a daemon restart every restored
/// worker reads idle before its script reports (an operator restart,
/// never a crash), and restoring 14 workers plus brain back-off can take
/// minutes, so no time window fits. Later sightings count normally.
pub fn first_idle_since_start(account: &str) -> bool {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    SEEN.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string())
}

fn operator_stops() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static STOPS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    STOPS.get_or_init(Default::default)
}

/// The operator unloaded, closed or (re)opened `account`'s worker: its
/// active node goes idle without a report, which is a stop, never a crash.
pub fn note_operator_stop(account: &str) {
    operator_stops().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string());
}

/// True (once) when the idle worker follows an operator stop.
pub fn take_operator_stop(account: &str) -> bool {
    operator_stops().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account)
}

#[cfg(test)]
mod tests {
    use super::{note_operator_stop, take_operator_stop};

    /// One stop excuses one report-less idle, never two.
    #[test]
    fn a_stop_flag_is_taken_once() {
        note_operator_stop("grace_probe");
        assert!(take_operator_stop("grace_probe"));
        assert!(!take_operator_stop("grace_probe"), "a later crash counts");
    }
}
