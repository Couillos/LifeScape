#![forbid(unsafe_code)]
//! The pure model of the stats (layering step D, 2026-11-24): report
//! contract and parsing, flow causes, the yield estimator, the error
//! model. No global, no thread, no service: functions and types over
//! values. `orch-session` (the worker) depends on this crate only;
//! `orch-stats` keeps the services (rates service, guides, holdings,
//! hiscores, xp) and re-exports these modules at their old paths.
pub mod errors_aggregate;
pub mod errors_model;
pub mod flow_causes;
pub mod rates_estimator;
pub mod rates_parse;
pub mod report_contract;
pub mod report_flows;
pub mod report_meta;
pub mod report_progress;
pub use lifescape_orch_core::{paths, logln, log_markers, error_stamp};
pub use lifescape_orch_world::zones;
