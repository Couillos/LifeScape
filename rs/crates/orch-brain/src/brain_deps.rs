#![forbid(unsafe_code)]
//! The handles a brain pass reads the world through (lot 3.5 of the
//! architecture plan; analysis 2.3 "injection de dépendances par
//! structure"): the market service, the measured rates, the published
//! guides, the fleet's crowd and the clock, taken once when the pass opens
//! and handed down, instead of 52 calls of `market_service::shared()`, 9 of
//! `guides_store::current()`, 6 of `live_snapshot()` and their kin, each
//! one a lock on a global.
//!
//! `BrainDeps::live()` reads the daemon's singletons once. The tests build
//! their own (`BrainDeps::with_market(..)`): a market service of their own,
//! a crowd and a clock they control, no thread-local nor process-wide state
//! to install. A module converted to `&BrainDeps` no longer names the
//! singletons; the ones left are listed in `docs/orchestrator.md`.

use std::sync::Arc;

use lifescape_orch_market::market_service::MarketService;

use crate::guides_store::GuideSet;
use crate::rates_service::Snapshot;

/// A site of the world: `(x, z, level)`.
pub(crate) type Site = (i64, i64, i64);

/// The clock the brain decides on.
pub(crate) trait Clock: Send + Sync {
    /// Unix seconds now.
    fn now_secs(&self) -> i64;
    /// Wall milliseconds of one game tick (`market_eta::live_tick_ms`).
    fn tick_ms(&self) -> u64;
}

/// The fleet's sites and seats, as `site_crowd` keeps them.
pub(crate) trait Crowd: Send + Sync {
    /// Sites the other accounts work, with their head count.
    fn others(&self, account: &str) -> Vec<(Site, u32)>;
    /// Seats engaged or worked by the others: site, method, head count.
    fn seats(&self, account: &str) -> Vec<(Site, String, u32)>;
}

/// The wall clock and the daemon's measured tick length.
pub(crate) struct WallClock;

impl Clock for WallClock {
    fn now_secs(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
    }

    fn tick_ms(&self) -> u64 {
        crate::market_eta::live_tick_ms()
    }
}

/// The daemon's `site_crowd` table.
pub(crate) struct FleetCrowd;

impl Crowd for FleetCrowd {
    fn others(&self, account: &str) -> Vec<(Site, u32)> {
        crate::site_crowd::others(account)
    }

    fn seats(&self, account: &str) -> Vec<(Site, String, u32)> {
        crate::site_crowd::seats(account)
    }
}

/// Everything a pass reads that is not its own job's state.
#[derive(Clone)]
pub(crate) struct BrainDeps {
    /// The market service; `None` while it is down (no data, no decision).
    pub market: Option<Arc<MarketService>>,
    /// The measured rates (an `Arc` snapshot, never a copy).
    pub rates: Arc<Snapshot>,
    /// The last published guides, `None` before the first build.
    pub guides: Option<Arc<GuideSet>>,
    pub crowd: Arc<dyn Crowd>,
    pub clock: Arc<dyn Clock>,
}

impl BrainDeps {
    /// The daemon's handles, read now (once per pass).
    pub(crate) fn live() -> Self {
        BrainDeps { market: crate::market_service::shared(), rates: crate::rates_service::live_snapshot(), guides: crate::guides_store::current(), crowd: Arc::new(FleetCrowd), clock: Arc::new(WallClock) }
    }

    /// Handles with a market of the caller's (tests): the rates and the
    /// guides as the process holds them, the live crowd and clock unless
    /// replaced (`with_crowd`, `with_clock`).
    #[cfg(test)]
    pub(crate) fn with_market(market: Option<Arc<MarketService>>) -> Self {
        BrainDeps { market, ..BrainDeps::live() }
    }

    #[cfg(test)]
    pub(crate) fn with_crowd(mut self, crowd: Arc<dyn Crowd>) -> Self {
        self.crowd = crowd;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
}
