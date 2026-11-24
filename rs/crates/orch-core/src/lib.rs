#![forbid(unsafe_code)]
/// Daemon log line on stdout (redirected to `run/daemon.log`), prefixed
/// with the UTC wall clock so a lot can be read as a timeline. CLI
/// commands keep plain `println!` (their stdout is the operator's).
/// Each line is also offered to the registered log sink (`log_sink`:
/// the errors collector and the idle reasons, brain abandons, stuck
/// graphs, bans; the binary registers it before any thread starts).
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        $crate::log_sink::emit(&line);
        println!("[{}] {}", $crate::daemon_clock(), line)
    }};
}

/// UTC `HH:MM:SS` for daemon log lines (same clock as session logs).
pub fn daemon_clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

pub mod config;
pub mod paths;
pub mod clock;
pub mod downtime;
pub mod log_sink;
pub mod client;
pub mod log_markers;
pub mod cpu_profile;
pub mod error_stamp;
pub mod log_rotate;
pub mod disk_guard;
pub mod res_stats;
pub mod store_pool;
pub mod names;
pub mod api_shapes;
pub mod nav_profile;
pub mod thread_profile;
pub mod economy_guard;
