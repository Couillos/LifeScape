#![forbid(unsafe_code)]
/// Daemon log line on stdout (redirected to `run/daemon.log`), prefixed
/// with the UTC wall clock so a lot can be read as a timeline. CLI
/// commands keep plain `println!` (their stdout is the operator's).
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        println!("[{}] {}", $crate::daemon_clock(), format_args!($($arg)*))
    };
}

/// UTC `HH:MM:SS` for daemon log lines (same clock as session logs).
pub fn daemon_clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}


pub mod paths;
pub mod brain;
pub mod brain_fold;
pub mod brain_market;
pub mod brain_plan;
pub mod brain_sales;
pub mod brain_wishlist;
pub mod brain_succession;
pub mod brain_tutorial;
pub mod market_convene;
pub mod market_eta;
pub mod market_daemon;
pub mod market_reprice;
pub mod market_service;
pub mod market_seed;
#[cfg(test)]
#[path = "market_service_tests.rs"]
mod market_service_tests;
pub mod jobs;
pub mod holdings;
pub mod registry;
pub mod mapcache;
pub mod mapbuild;
pub mod mapcli;
pub mod minimap;
pub mod plan_service;
pub mod png;
pub mod web;
pub mod web_items;
pub mod web_job_lives;
pub mod web_ws;
pub mod web_lives;
pub mod web_market;
pub mod zones;
pub mod cli;
pub mod await_cmd;
pub mod verify_assert;
pub mod verify_cmd;
pub mod verify_scenario;
pub mod verify_reject;
pub mod daemon;
pub mod daemon_watch;
pub mod daemon_conn;
pub mod supervise;
pub mod session;
pub mod session_cmds;
pub mod dump_npc_ranges;
pub mod dump_transitions;
pub mod obtaincmd;
pub mod routecmd;
pub mod replaynav;
pub mod session_calls;
pub mod session_push;
pub mod session_heartbeat;
pub mod session_report;
pub mod session_rescue;
pub mod staticmap;
pub mod res_stats;
pub mod admission;
pub mod api_shapes;
pub mod push_hub;
pub mod push_conn;
pub mod rates_price;
pub mod rates_service;
pub mod heartbeat_flows;
pub mod session_log;
pub mod store_queue;
pub mod daemon_pool;
pub mod bench_offline;
