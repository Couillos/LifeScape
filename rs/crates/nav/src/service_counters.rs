#![forbid(unsafe_code)]
//! Every counter of the navigation service in one object (lot N6).
//!
//! They sat in `Service` beside ten locks and the queue: about thirty
//! atomics, read by `stats`, the `stats` op and the daemon gauge. Grouped
//! here they can be bumped, read and tested without a socket, a snapshot
//! or a thread (`Counters::default()` is all zero). Atomics only, never a
//! lock: any thread bumps any counter at any time.

use std::sync::atomic::{AtomicU32, AtomicU64};

#[derive(Default)]
pub(crate) struct Counters {
    /// Route-cache style outcomes of ticketed plans and cost batches.
    pub(crate) hits: AtomicU64,
    pub(crate) misses: AtomicU64,
    pub(crate) joins: AtomicU64,
    /// L2 edge set found / built.
    pub(crate) l2_hits: AtomicU64,
    pub(crate) l2_miss: AtomicU64,
    /// Admission refusals: account cap, full queue, stale version.
    pub(crate) refused_cap: AtomicU64,
    pub(crate) refused_full: AtomicU64,
    pub(crate) refused_stale: AtomicU64,
    pub(crate) unknown_polls: AtomicU64,
    /// RAM gate (§5.9): warnings (L1 halved) and sheds (submit refused).
    pub(crate) ram_warn: AtomicU64,
    pub(crate) ram_shed: AtomicU64,
    /// Direct-read batches served, one per op.
    pub(crate) door_defs_served: AtomicU64,
    pub(crate) tiles_flags_served: AtomicU64,
    pub(crate) static_locs_served: AtomicU64,
    pub(crate) f2p_served: AtomicU64,
    pub(crate) toll_fence_served: AtomicU64,
    pub(crate) wants_served: AtomicU64,
    pub(crate) diff_served: AtomicU64,
    pub(crate) debug_served: AtomicU64,
    pub(crate) transitions_list_served: AtomicU64,
    pub(crate) probe_targets_served: AtomicU64,
    pub(crate) doors_near_served: AtomicU64,
    /// Ticket traffic.
    pub(crate) submits_served: AtomicU64,
    pub(crate) polls_served: AtomicU64,
    /// `walk_costs` batches served, and those answered by the costs
    /// cache (no search).
    pub(crate) walk_costs_served: AtomicU64,
    pub(crate) walk_costs_hits: AtomicU64,
    /// Connections served by the `plan-query` pool (never the acceptor).
    pub(crate) pooled: AtomicU64,
    /// N8 detour: excess over the cheapest served cost for a pair.
    pub(crate) detour_sum: AtomicU64,
    pub(crate) detour_max: AtomicU32,
    pub(crate) detour_plans: AtomicU64,
    /// Short hops answered by the fast path.
    pub(crate) fast: crate::service_fast::FastCounters,
    /// Per-kind request meter (lot N0): count, thread CPU, tiles visited.
    pub(crate) meter: crate::nav_meter::Meter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn counters_start_at_zero_and_count_without_a_service() {
        let counters = Counters::default();
        assert_eq!(counters.hits.load(Ordering::Relaxed), 0);
        counters.hits.fetch_add(2, Ordering::Relaxed);
        counters.detour_max.fetch_max(7, Ordering::Relaxed);
        counters.detour_max.fetch_max(3, Ordering::Relaxed);
        assert_eq!(counters.hits.load(Ordering::Relaxed), 2);
        assert_eq!(counters.detour_max.load(Ordering::Relaxed), 7);
        assert_eq!(counters.meter.read()[0].count, 0);
    }
}
