#![forbid(unsafe_code)]
//! Yield rates (plan rendements R2/R3): from one finished task engagement
//! to a published per-6000-ticks figure the planner and the page read.
//!
//! Pipeline: the worker pushes a `TaskReport` JSON value (contract 2,
//! `report_contract`); `jobs::ingest_report_into` stores the raw rows in
//! `lifescape-store` (never edited here), then `record_snapshot` folds it
//! into the live `Snapshot`:
//! - contract-2 reports feed the yield estimator (`rates_measure` admits
//!   them, `rates_estimator` keeps decayed sums per D2 key, `rates_read`
//!   serves estimates, priors and the derived XP); contract-1 reports
//!   (method = script) are never aggregated any more;
//! - trading methods (`market`, `market:*`, `shop:*`, D44) feed the
//!   merchant book (`trading`) and never a yield;
//! - the tutorial feeds nothing.
//!
//! The snapshot lives in memory behind an `Arc` (a reader takes the
//! `Arc`, never a copy), is rewritten to `run/rates-snapshot.json` at
//! most once per minute when dirty (`rates_flush`, plus a flush at
//! graceful shutdown), reloads at restart, and rebuilds from the store's
//! contract-2 reports when the file is missing, corrupt or of an older
//! schema (`rates_snapshot_io`). Its size is bounded by the number of
//! keys (`rates_estimator::MAX_BUCKETS`), never by the number of runs.

use std::sync::Arc;

#[path = "rates_trading.rs"]
mod trading;
#[path = "web_rates.rs"]
mod web_rates;
#[path = "rates_parse.rs"]
mod parse;
#[path = "rates_snapshot_io.rs"]
mod snapshot_io;

pub use parse::*;
pub use snapshot_io::{rebuild_from_store, snapshot_path};
pub(crate) use snapshot_io::write_snapshot_file;
#[cfg(test)]
pub(crate) use snapshot_io::snapshot_from_json;
pub use web_rates::{measure_rows, rates_api, rates_payload, trading_api};
use snapshot_io::global_snapshot;

use crate::rates_estimator::YieldBook;
use crate::rates_measure::{admit, xp_gaps, Admission};
use crate::report_contract::ParsedContract;

/// Ticks of one normalised engagement window (one in-game hour in prod).
pub const NORMALIZE_TICKS: f64 = 6000.0;
/// Width of a level band: bands are 1-5, 6-10, ...
pub const LEVEL_BAND: i64 = 5;
/// Runs from which a measure counts (page AND brain, one threshold).
pub const MIN_MEASURED_REPORTS: usize = 3;

/// Whole rates state: the yield estimator and the merchant book.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub(crate) book: YieldBook,
    pub(crate) trades: trading::TradeBook,
}

impl Snapshot {
    pub fn new() -> Self {
        Snapshot::default()
    }

    /// The yield estimator's buckets.
    pub fn book(&self) -> &YieldBook {
        &self.book
    }

    /// Folds one report in: trading runs into the book, contract-2 runs
    /// into the estimator (admission logged), contract-1 runs nowhere.
    /// Returns what the estimator did with it.
    pub fn record(&mut self, parsed: &ParsedReport, contract: Option<&ParsedContract>, clock: i64) -> Option<Admission> {
        if is_trading_method(&parsed.method) {
            self.trades.record_trade(parsed);
            return None;
        }
        if is_unrated_method(&parsed.method) {
            return None;
        }
        let Some(contract) = contract else {
            crate::logln!("rates: {} report of {} is contract 1, never aggregated", parsed.account, parsed.method);
            return None;
        };
        let admission = admit(contract, &parsed.stop, clock);
        self.apply(&admission, &parsed.account);
        if matches!(admission, Admission::Rated(_, _)) {
            let observed: Vec<(String, i64)> = parsed.xp.iter().map(|gain| (gain.skill.clone(), gain.amount)).collect();
            for gap in xp_gaps(contract, &observed) {
                crate::logln!(
                    "rates: xp coherence {} {} {}: derived {:.1} observed {:.1} (alert above {:.0} %)",
                    parsed.account,
                    contract.planner_method,
                    gap.skill,
                    gap.derived,
                    gap.observed,
                    crate::rates_measure::XP_GAP_ALERT * 100.0
                );
            }
        }
        Some(admission)
    }

    /// Applies an admission to the estimator (rebuild and live share it).
    pub(crate) fn apply(&mut self, admission: &Admission, account: &str) {
        match admission {
            Admission::Rated(key, sample) => self.book.record(key, sample),
            Admission::Failed(key, ticks) => self.book.record_failure(key, *ticks),
            Admission::Rejected(key, reason) => {
                crate::logln!("rates: {account} {} sample rejected above the physical ceiling: {reason}", key.method);
                self.book.record_rejected(key);
            }
            Admission::Skipped(reason) => crate::logln!("rates: {account} report not aggregated: {reason}"),
        }
    }
}

/// Wall clock in seconds (bucket ages).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// The live snapshot, shared (an `Arc` clone, never a copy of the
/// buckets): read-only planning input, public yields only.
pub fn live_snapshot() -> Arc<Snapshot> {
    global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

pub use crate::rates_price::price_estimate;

/// Snapshot generation: bumped by every recorded report (the rates page
/// index rebuilds when it changes, `web_rates_view`). Moved here when
/// the old profit table (`rates_profit`) left for the money guide.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Current snapshot generation.
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Records one report in the live snapshot and marks it dirty: the
/// `rates-flush` thread persists it at most once per
/// `rates_flush::FLUSH_PERIOD`, the daemon's graceful exit flushes the
/// rest (crash/SIGTERM loses at most that window, see `rates_flush`).
/// Copy on write: a reader holding the previous `Arc` keeps it.
pub fn record_snapshot(parsed: &ParsedReport, contract: Option<&ParsedContract>, clock: i64) -> Result<(), String> {
    {
        let mut guard = global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::make_mut(&mut guard).record(parsed, contract, clock);
    }
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    crate::rates_flush::mark_dirty();
    Ok(())
}

#[cfg(test)]
#[path = "rates_service_tests.rs"]
mod rates_tests;
