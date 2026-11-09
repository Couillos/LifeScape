#![forbid(unsafe_code)]
//! Walk-cost sobriety (E14): group identical destinations, count the
//! remote calls. The service L1 stays the cross-call cache (a repeat
//! still hits L1, never recomputes); this groups WITHIN one call, where
//! spot searches used to pay one submit+poll per duplicate tile.
//!
//! Attribution of the measured ~1.5 demands/s/bot (L0): spot search
//! (`kit/spots.rs`, `kit/trees.rs`) batches `walk_costs` per candidate,
//! walking legs replan once per failure, reprises resubmit after skew.
//! One `plan` op (E3) now serves each cost in a single round-trip, and
//! duplicates inside a batch cost zero.

use crate::client::Bot;

/// Per-bot walk-cost counters, read by `cost_stats` (E13 gauge input).
#[derive(Debug, Default)]
pub(crate) struct CostStats {
    pub remote_calls: u64,
    pub dedup_hits: u64,
}

impl Bot {
    /// (remote cost calls, in-batch duplicate hits) since session start.
    pub fn cost_stats(&self) -> (u64, u64) {
        (self.cost_stats.remote_calls, self.cost_stats.dedup_hits)
    }
}

#[cfg(test)]
mod tests {
    use super::CostStats;

    /// Counters start at zero and add up.
    #[test]
    fn counters_accumulate() {
        let mut stats = CostStats::default();
        assert_eq!((stats.remote_calls, stats.dedup_hits), (0, 0));
        stats.remote_calls += 2;
        stats.dedup_hits += 1;
        assert_eq!((stats.remote_calls, stats.dedup_hits), (2, 1));
    }
}
