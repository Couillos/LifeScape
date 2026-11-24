#![forbid(unsafe_code)]
//! The configuration block the daemon prints at startup (lot 5.10): the
//! environment knobs (`Config::lines`) and the timeouts and cadences that
//! are constants of the code, read from the constants themselves (so the
//! printed value cannot drift from the one in use).

use lifescape_orch_core::config::Config;
use lifescape_orch_daemon::{daemon, daemon_watch, status_poll};

/// The timeouts and cadences, as the code holds them (no environment
/// variable moves any of them).
pub fn constant_lines() -> Vec<String> {
    vec![
        format!("config worker_call_timeout={}s (constant)", lifescape_orch_core::client::WORKER_TIMEOUT),
        format!("config open_timeout={}s (constant)", daemon::OPEN_TIMEOUT),
        format!("config brain_safety_tick={}s (constant)", lifescape_orch_brain::brain::SAFETY_TICK_SECS),
        format!("config status_silence={}s backstop={}s transition_gap={}s silent_gap={}s (constant)", status_poll::SILENCE_SECS, status_poll::BACKSTOP_SECS, status_poll::TRANSITION_GAP_SECS, status_poll::SILENT_GAP_SECS),
        format!("config watchdog_stale={}s window={}s max_restarts={} (constant)", daemon_watch::WATCHDOG_STALE_SECS, daemon_watch::WATCHDOG_WINDOW_SECS, daemon_watch::WATCHDOG_MAX_RESTARTS),
        "config server_tick=measured live (market_eta::live_tick_ms; 600 ms until the first sample)".to_string(),
    ]
}

/// Every line of the startup block.
pub fn lines(config: &Config) -> Vec<String> {
    let mut lines = config.lines();
    lines.extend(constant_lines());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Today's values (before lot 5.10 they were read where they were
    /// used): the printed block is exactly these.
    #[test]
    fn the_printed_defaults_are_todays_constants() {
        assert_eq!(
            lines(&Config::default()),
            vec![
                "config LIFESCAPE_SERVER_HOST=127.0.0.1 (default)",
                "config LIFESCAPE_SERVER_PORT=43594 (default)",
                "config LIFESCAPE_WEB_PORT=8765 (default)",
                "config LIFESCAPE_REDRAW_GATE=shadow (default)",
                "config LIFESCAPE_RATES_PATH=run/rates-snapshot.json (default)",
                "config worker_call_timeout=40s (constant)",
                "config open_timeout=150s (constant)",
                "config brain_safety_tick=10s (constant)",
                "config status_silence=5s backstop=60s transition_gap=2s silent_gap=5s (constant)",
                "config watchdog_stale=45s window=300s max_restarts=3 (constant)",
                "config server_tick=measured live (market_eta::live_tick_ms; 600 ms until the first sample)",
            ]
        );
    }
}
