use super::{build_ring, convenable, grouping_secs, pair_tiles, stagger, sweep_after_secs};
use crate::market_eta::zone_of;
use std::collections::{HashMap, HashSet};

fn open_square(tile_open: &dyn Fn(i32, i32) -> bool) -> Vec<(i64, i64)> {
    build_ring(tile_open)
}

/// Ring skirts the basin and drops blocked tiles: fountain water and
/// the four busts never host a handover.
#[test]
fn ring_skirts_basin_and_busts() {
    let busts: HashSet<(i32, i32)> = [(3210, 3429), (3213, 3426), (3213, 3432), (3216, 3429)]
        .into_iter()
        .collect();
    let ring = open_square(&|x, z| {
        let basin = (3211..=3215).contains(&x) && (3427..=3431).contains(&z);
        !basin && !busts.contains(&(x, z))
    });
    assert!(!ring.is_empty(), "square hosts handovers");
    for (x, z) in &ring {
        let wet = (3211..=3215).contains(x) && (3427..=3431).contains(z);
        assert!(!wet, "no basin tile {x},{z}");
        assert!(!busts.contains(&(*x as i32, *z as i32)), "no bust tile {x},{z}");
    }
    // Four sides minus four busts, then the outer ring fills past eight.
    assert!(ring.len() > 8, "second ring extends past eight, got {}", ring.len());
    // Adjacent pairs tile consecutively: the first two are neighbours.
    let (x0, y0) = ring[0];
    let (x1, y1) = ring[1];
    assert!((x0 - x1).abs() <= 1 && (y0 - y1).abs() <= 1, "ring order walks, starts {x0},{y0} {x1},{y1}");
}

/// Full square queues instead of stacking: no pair, no random tile.
#[test]
fn full_ring_queues_without_stacking() {
    let ring = open_square(&|_, _| true);
    let busy: HashSet<(i64, i64)> = ring.iter().cloned().collect();
    assert_eq!(pair_tiles(&ring, &busy), None, "full ring queues");
    let mut one_free = busy.clone();
    one_free.remove(&ring[0]);
    assert_eq!(pair_tiles(&ring, &one_free), None, "one tile is no pair");
}

/// Pairs are adjacent and never double-booked.
#[test]
fn pairs_are_adjacent_and_exclusive() {
    let ring = open_square(&|_, _| true);
    let mut busy = HashSet::new();
    let first = pair_tiles(&ring, &busy).expect("free pair");
    assert!((first.0.0 - first.1.0).abs() <= 1 && (first.0.1 - first.1.1).abs() <= 1, "adjacent pair");
    busy.insert(first.0);
    busy.insert(first.1);
    let second = pair_tiles(&ring, &busy).expect("second pair");
    assert_ne!(first.0, second.0, "no double booking");
    assert_ne!(first.0, second.1, "no double booking");
}

/// Grouping window holds matches, then releases them together.
#[test]
fn grouping_holds_then_releases() {
    let window = grouping_secs(60);
    assert!(!convenable(100.0, 100.0 + window - 1.0, 60), "window holds");
    assert!(convenable(100.0, 100.0 + window, 60), "window releases");
    assert!(convenable(100.0, 100.0 + window + 5000.0, 60), "late still convenes");
    assert!(sweep_after_secs(400, 60) > 400.0 * 60.0 / 1000.0, "slack on top");
}

/// Stagger: the farther bot leaves first, both land together, the
/// deadline is twice the longest leg plus margin.
#[test]
fn stagger_lands_together_before_deadline() {
    let (depart_near, end_near, depart_far, end_far) = stagger(1000, 50, 5000, 200);
    assert!(depart_far - 5000 < depart_near - 1000, "farther leaves first in wall terms");
    assert_eq!(depart_far + 200 - 5000, depart_near + 50 - 1000, "simultaneous arrival");
    assert_eq!(end_near, 1000 + super::PREP_MARGIN_TICKS + 400 + super::DEADLINE_MARGIN_TICKS);
    assert_eq!(end_far, 5000 + super::PREP_MARGIN_TICKS + 400 + super::DEADLINE_MARGIN_TICKS);
    let (same_a, same_end_a, same_b, same_end_b) = stagger(1000, 80, 1000, 80);
    assert_eq!(same_a, same_b, "equal legs leave together");
    assert!(same_end_a > same_a + 80, "deadline past arrival");
    assert_eq!(same_end_a, same_end_b, "same epoch same deadline");
}

/// Zones quantize positions 32 by 32 for the ETA cache.
#[test]
fn zones_quantize_32_wide() {
    assert_eq!(zone_of(3213, 3429), zone_of(3220, 3435), "same square one zone");
    assert_ne!(zone_of(3213, 3429), zone_of(3100, 3429), "far x another zone");
}

/// Ring against the real snapshot (same loader as the daemon): the
/// square hosts pairs around the basin, none on water, every tile
/// inside the two surveyed rectangles.
#[test]
fn ring_against_snapshot() {
    let snapshot = crate::plan_service::load_snapshot().expect("daemon snapshot loads");
    let ring = super::build_ring_from_snapshot(&snapshot);
    assert!(ring.len() >= 8, "square hosts pairs, got {}", ring.len());
    for (x, z) in &ring {
        let basin = (3211..=3215).contains(x) && (3427..=3431).contains(z);
        assert!(!basin, "no water tile {x},{z}");
        let inner = (3210..=3216).contains(x) && (3426..=3432).contains(z);
        let outer = (3209..=3217).contains(x) && (3425..=3433).contains(z);
        assert!(inner || outer, "tile {x},{z} inside the surveyed rects");
    }
    let busy = HashSet::new();
    let pair = pair_tiles(&ring, &busy).expect("a free pair on the live square");
    assert!((pair.0.0 - pair.1.0).abs() <= 1 && (pair.0.1 - pair.1.1).abs() <= 1, "live pair adjacent");
}

/// Relist rows stamp the consumed ranks: resting keeps its queue
/// position, the taker its arrival rank.
#[test]
fn relist_rows_keep_consumed_ranks() {
    use lifescape_market::{Book, Side};
    let mut book = Book::new();
    book.place(1, "maker", 314, Side::Sell, 10, 4).expect("maker rests");
    let pre: HashMap<u64, lifescape_market::Order> = book.orders().into_iter().map(|order| (order.id, order)).collect();
    let taker_seq = book.next_seq();
    let (fills, _) = book.place(2, "taker", 314, Side::Buy, 10, 5).expect("taker fills");
    assert_eq!(fills.len(), 1, "one fill");
    let rows = super::relist_for_fill(&pre, taker_seq, Side::Buy, "taker", 5, &fills[0]);
    assert_eq!(rows.len(), 2, "resting plus taker rows");
    assert_eq!(rows[0].seq, 0, "maker keeps its head rank");
    assert_eq!(rows[1].seq, taker_seq, "taker keeps its arrival rank");
    // Relist restores the head of the queue, not the tail.
    book.place(3, "later", 314, Side::Sell, 10, 4).expect("later rests");
    for mut row in rows {
        row.id = 100 + row.seq;
        book.relist(row);
    }
    let head = book.orders().into_iter().find(|order| order.account == "maker").expect("maker back");
    assert_eq!(head.seq, 0, "maker back at its rank");
}

/// The tick meter starts prod-safe and converges on the live rate from
/// two sightings; stale or duplicate ticks never move it.
#[test]
fn tick_meter_converges_on_live_rate() {
    use super::TickMeter;
    let mut meter = TickMeter::new();
    assert_eq!(meter.observe(1000.0, 100), 600, "prod-safe start");
    let ms = meter.observe(1000.0 + 60.0 * 20.0 / 1000.0, 120);
    assert!(ms < 600 && ms > 60, "twenty dev ticks pull it down, got {ms}");
    for round in 1..30u64 {
        meter.observe(1000.0 + 60.0 * 20.0 / 1000.0 + round as f64 * 0.06, 120 + round);
    }
    let ms = meter.observe(1000.0 + 60.0 * 20.0 / 1000.0 + 30.0 * 0.06, 150);
    assert!((ms as i64 - 60).abs() <= 15, "converges near 60 ms, got {ms}");
    let frozen = meter.observe(2000.0, 150);
    assert_eq!(frozen, ms, "duplicate ticks never move it");
}
