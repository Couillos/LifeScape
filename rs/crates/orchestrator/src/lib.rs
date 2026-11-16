#![forbid(unsafe_code)]
/// Daemon log line on stdout (redirected to `run/daemon.log`), prefixed
/// with the UTC wall clock so a lot can be read as a timeline. CLI
/// commands keep plain `println!` (their stdout is the operator's).
/// Each line is also offered to the errors collector (brain abandons,
/// stuck graphs, bans: a no-op until the daemon starts it).
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        $crate::errors_ingest::observe_log_line(&line);
        println!("[{}] {}", $crate::daemon_clock(), line)
    }};
}

/// UTC `HH:MM:SS` for daemon log lines (same clock as session logs).
pub fn daemon_clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}


pub mod paths;
pub mod brain;
pub mod brain_alch;
pub mod brain_ambition;
pub mod brain_await;
pub mod brain_chain;
pub mod brain_idle;
pub mod brain_bids;
#[cfg(test)]
mod brain_bids_sim_tests;
#[cfg(test)]
mod live_check_tests;
#[cfg(test)]
mod loot_check_tests;
pub mod economy_guard;
pub mod brain_fold;
pub mod brain_gear;
pub mod brain_gear_hold;
pub mod brain_grace;
pub mod brain_filler;
pub mod brain_launch;
pub mod brain_locks;
pub mod brain_loot;
pub mod brain_market;
pub mod brain_npc_sale;
pub mod brain_plan;
pub mod brain_quest_gate;
pub mod brain_rescue;
pub mod brain_sales;
pub mod brain_reserve;
pub mod brain_wishlist;
pub mod brain_succession;
pub mod brain_stuck;
pub mod brain_train;
pub mod brain_tutorial;
pub mod market_absent;
pub mod market_convene;
pub mod market_convene_sweep;
pub mod market_eta;
pub mod create_user_cmd;
pub mod market_lineage;
pub mod market_maker;
pub mod market_maker_report;
pub mod market_daemon;
pub mod market_claims;
pub mod market_demand;
pub mod market_reprice;
pub mod market_seller;
pub mod market_service;
pub mod market_audit;
pub mod market_merge;
pub mod market_prints;
pub mod market_uncross;
pub mod market_orders;
#[cfg(test)]
pub(crate) mod market_test_support;
pub mod market_seed;
#[cfg(test)]
#[path = "market_service_tests.rs"]
mod market_service_tests;
#[cfg(test)]
#[path = "market_crossing_tests.rs"]
mod market_crossing_tests;
#[cfg(test)]
#[path = "market_commit_tests.rs"]
mod market_commit_tests;
#[cfg(test)]
#[path = "market_history_tests.rs"]
mod market_history_tests;
#[cfg(test)]
#[path = "economy_sim_tests.rs"]
mod economy_sim_tests;
#[cfg(test)]
#[path = "market_integrity_tests.rs"]
mod market_integrity_tests;
#[cfg(test)]
#[path = "market_uncross_tests.rs"]
mod market_uncross_tests;
#[cfg(test)]
#[path = "market_taker_tests.rs"]
mod market_taker_tests;
#[cfg(test)]
#[path = "market_impact_tests.rs"]
mod market_impact_tests;
pub mod jobs;
pub mod holdings;
pub mod hiscores;
pub mod hiscores_service;
pub mod registry;
pub mod mapcache;
pub mod mapbuild;
pub mod mapcli;
pub mod minimap;
pub mod plan_service;
pub mod nav_profile;
pub mod png;
pub mod web;
pub mod web_items;
pub mod web_job_lives;
pub mod web_ws;
pub mod web_lives;
pub mod web_brain;
pub mod web_market;
pub mod web_errors;
pub mod errors_aggregate;
pub mod errors_ingest;
pub mod errors_model;
pub mod zones;
pub mod bid_why;
pub mod cli;
pub mod rares_cmd;
pub mod await_cmd;
pub mod verify_assert;
pub mod verify_cmd;
pub mod method_check;
pub mod method_health;
pub mod economy_cmd;
pub mod verify_scenario;
pub mod verify_reject;
pub mod daemon;
pub mod daemon_watch;
pub mod daemon_conn;
pub mod cpu_profile;
#[cfg(test)]
#[path = "cpu_bench_tests.rs"]
mod cpu_bench_tests;
pub mod daemon_close;
pub mod supervise;
pub mod site_crowd;
pub mod session;
pub mod session_cmds;
pub mod session_locks;
pub mod dump_npc_combat;
pub mod dump_action_odds;
pub mod dump_spell_targets;
pub mod dump_npc_ranges;
pub mod dump_transitions;
pub mod names;
pub mod obtaincmd;
pub mod routecmd;
pub mod replaynav;
pub mod session_calls;
pub mod session_push;
pub mod session_heartbeat;
pub mod session_report;
pub mod report_flows;
pub mod flow_causes;
pub mod report_meta;
pub mod report_contract;
pub mod purge_cmd;
pub mod rates_rebuild_cmd;
pub mod web_guides;
pub mod report_progress;
pub mod session_rescue;
pub mod staticmap;
pub mod res_stats;
pub mod disk_guard;
pub mod api_shapes;
pub mod push_hub;
pub mod push_conn;
pub mod rates_flush;
pub mod rates_price;
pub mod rates_service;
pub mod rates_estimator;
pub mod rates_measure;
pub mod rates_names;
pub mod rates_read;
pub mod heartbeat_flows;
pub mod session_log;
pub mod log_rotate;
pub mod brain_tools;
pub mod brain_tool_shop;
pub mod store_queue;
pub mod daemon_pool;
pub mod worker_owner;
pub mod bench_offline;
