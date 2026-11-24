#![forbid(unsafe_code)]
//! Per-request-kind meter of the navigation service (lot N0, 2026-11-24).
//!
//! The `cpu-profile nav` line said how much CPU the service threads used,
//! never on what. This meter counts, per kind of request, how many were
//! served, the thread CPU they took (`/proc/thread-self/schedstat`, ns
//! on the CPU, not wall time) and the tiles their searches visited; for
//! `walk_costs`, how many searches ended without reaching every target
//! (cut by the cost horizon, or the reachable area ran out first) and the
//! tiles those weighed. It reads state and never changes a search.
//!
//! The searches report their visits through a thread-local tally
//! (`add_visits`, `note_unreached`): the probe around a request reads the
//! tally before and after, so a request that runs several searches (a
//! plan: up to four) sums them without any argument threading.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a request was, for the per-kind rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Multi-target price batch (`walk_costs`).
    WalkCosts,
    /// Short hop answered by `fast_hop` (no planner).
    FastServed,
    /// Short hop `fast_hop` could not prove and handed to the planner:
    /// the cost of the attempt only.
    FastDeclined,
    /// Plan computed on the pool (cache miss): the Dijkstra legs.
    PlanComputed,
    /// Grouped reads: door defs, tile flags, locs, F2P, wants, diff,
    /// transitions, probe targets, doors near.
    Grouped,
    /// Diagnosis dossier (`debug`).
    Debug,
    /// Toll fence lookup.
    TollFence,
    /// Ticket traffic (`submit`, `poll`, `cancel`) and `stats`.
    Other,
}

pub const KINDS: [Kind; 8] = [
    Kind::WalkCosts,
    Kind::FastServed,
    Kind::FastDeclined,
    Kind::PlanComputed,
    Kind::Grouped,
    Kind::Debug,
    Kind::TollFence,
    Kind::Other,
];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::WalkCosts => "walk_costs",
            Kind::FastServed => "fast",
            Kind::FastDeclined => "fast_declined",
            Kind::PlanComputed => "plan_computed",
            Kind::Grouped => "grouped",
            Kind::Debug => "debug",
            Kind::TollFence => "toll_fence",
            Kind::Other => "other",
        }
    }

    fn slot(self) -> usize {
        KINDS.iter().position(|kind| *kind == self).unwrap_or(KINDS.len() - 1)
    }
}

/// Totals of one kind since the service started.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct OpStat {
    pub count: u64,
    pub cpu_ns: u64,
    pub visits: u64,
    /// Searches that ended without reaching every target.
    pub unreached: u64,
    /// Tiles those searches visited.
    pub unreached_visits: u64,
    /// Among the unreached ones, those cut by the horizon (or the visit
    /// bound) with tiles still to expand, against the area running out.
    pub horizon_ends: u64,
}

/// One row per `KINDS` entry.
pub type OpStats = [OpStat; 8];

thread_local! {
    static VISITS: Cell<u64> = const { Cell::new(0) };
    static UNREACHED: Cell<(u64, u64, u64)> = const { Cell::new((0, 0, 0)) };
}

/// A search visited `count` tiles (called once per finished search).
pub(crate) fn add_visits(count: usize) {
    VISITS.with(|cell| cell.set(cell.get() + count as u64));
}

/// A multi-target search ended without reaching every target after
/// `visits` tiles; `horizon` when the cost horizon (or visit bound) cut
/// it rather than the reachable area running out.
pub(crate) fn note_unreached(visits: usize, horizon: bool) {
    UNREACHED.with(|cell| {
        let (count, tiles, cut) = cell.get();
        cell.set((count + 1, tiles + visits as u64, cut + u64::from(horizon)));
    });
}

/// CPU nanoseconds this thread has spent on a core (0 when unreadable).
fn thread_cpu_ns() -> u64 {
    std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()
        .and_then(|text| text.split_whitespace().next().and_then(|field| field.parse::<u64>().ok()))
        .unwrap_or(0)
}

/// Readings at the start of a request.
pub struct Probe {
    cpu_ns: u64,
    visits: u64,
    unreached: (u64, u64, u64),
}

impl Probe {
    pub fn start() -> Probe {
        Probe { cpu_ns: thread_cpu_ns(), visits: VISITS.with(Cell::get), unreached: UNREACHED.with(Cell::get) }
    }
}

#[derive(Default)]
struct Row {
    count: AtomicU64,
    cpu_ns: AtomicU64,
    visits: AtomicU64,
    unreached: AtomicU64,
    unreached_visits: AtomicU64,
    horizon_ends: AtomicU64,
}

/// Lock-free counters, shared by every service thread.
#[derive(Default)]
pub struct Meter {
    rows: [Row; 8],
}

impl Meter {
    /// Close a request of `kind` opened by `probe`.
    pub fn record(&self, kind: Kind, probe: Probe) {
        let row = &self.rows[kind.slot()];
        let (count, tiles, cut) = UNREACHED.with(Cell::get);
        row.count.fetch_add(1, Ordering::Relaxed);
        row.cpu_ns.fetch_add(thread_cpu_ns().saturating_sub(probe.cpu_ns), Ordering::Relaxed);
        row.visits.fetch_add(VISITS.with(Cell::get).saturating_sub(probe.visits), Ordering::Relaxed);
        row.unreached.fetch_add(count.saturating_sub(probe.unreached.0), Ordering::Relaxed);
        row.unreached_visits.fetch_add(tiles.saturating_sub(probe.unreached.1), Ordering::Relaxed);
        row.horizon_ends.fetch_add(cut.saturating_sub(probe.unreached.2), Ordering::Relaxed);
    }

    pub fn read(&self) -> OpStats {
        let mut out = [OpStat::default(); 8];
        for (stat, row) in out.iter_mut().zip(self.rows.iter()) {
            *stat = OpStat {
                count: row.count.load(Ordering::Relaxed),
                cpu_ns: row.cpu_ns.load(Ordering::Relaxed),
                visits: row.visits.load(Ordering::Relaxed),
                unreached: row.unreached.load(Ordering::Relaxed),
                unreached_visits: row.unreached_visits.load(Ordering::Relaxed),
                horizon_ends: row.horizon_ends.load(Ordering::Relaxed),
            };
        }
        out
    }
}

/// The `ops=` segment of the `cpu-profile nav` line from two readings:
/// per kind `name=count/cpu_ms/visits` (kinds idle in the minute are
/// left out), then the searches that ended unreached, for `walk_costs` (no target
/// left to settle) and for planned legs (`Closest`: the area ran out;
/// `OverBudget`: the visit cap or the deadline cut it).
pub fn ops_segment(before: &OpStats, after: &OpStats) -> String {
    let mut parts = Vec::new();
    for (index, kind) in KINDS.iter().enumerate() {
        let (was, now) = (&before[index], &after[index]);
        let count = now.count.saturating_sub(was.count);
        if count == 0 {
            continue;
        }
        parts.push(format!(
            "{}={}/{:.0}ms/{}v",
            kind.name(),
            count,
            now.cpu_ns.saturating_sub(was.cpu_ns) as f64 / 1e6,
            now.visits.saturating_sub(was.visits)
        ));
    }
    for (label, kind) in [("costs", Kind::WalkCosts), ("plan", Kind::PlanComputed)] {
        let slot = kind.slot();
        parts.push(format!(
            "{label}_unreached={}/{}v horizon_cut={}",
            after[slot].unreached.saturating_sub(before[slot].unreached),
            after[slot].unreached_visits.saturating_sub(before[slot].unreached_visits),
            after[slot].horizon_ends.saturating_sub(before[slot].horizon_ends)
        ));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_sums_the_searches_it_ran() {
        let meter = Meter::default();
        let probe = Probe::start();
        add_visits(100);
        add_visits(50);
        note_unreached(120, true);
        meter.record(Kind::WalkCosts, probe);
        let probe = Probe::start();
        add_visits(7);
        meter.record(Kind::FastServed, probe);
        let stats = meter.read();
        assert_eq!(stats[Kind::WalkCosts.slot()].count, 1);
        assert_eq!(stats[Kind::WalkCosts.slot()].visits, 150);
        assert_eq!(stats[Kind::WalkCosts.slot()].unreached, 1);
        assert_eq!(stats[Kind::WalkCosts.slot()].unreached_visits, 120);
        assert_eq!(stats[Kind::WalkCosts.slot()].horizon_ends, 1);
        assert_eq!(stats[Kind::FastServed.slot()].visits, 7);
        assert_eq!(stats[Kind::FastServed.slot()].unreached, 0);
    }

    #[test]
    fn segment_lists_busy_kinds_and_the_unreached_costs() {
        let before = OpStats::default();
        let mut after = OpStats::default();
        after[Kind::WalkCosts.slot()] = OpStat { count: 4, cpu_ns: 8_000_000, visits: 900, unreached: 1, unreached_visits: 600, horizon_ends: 1 };
        let text = ops_segment(&before, &after);
        assert_eq!(text, "walk_costs=4/8ms/900v costs_unreached=1/600v horizon_cut=1 plan_unreached=0/0v horizon_cut=0");
    }
}
