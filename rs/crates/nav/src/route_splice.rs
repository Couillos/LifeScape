#![forbid(unsafe_code)]
//! Sub-path and splice reuse of cached plans (CPU 2026-11-14).
//!
//! Runs OUTSIDE the cache lock on candidates taken out of it
//! (`RouteCache::candidates`). Two families:
//! - sub-paths (prefix / suffix / infix): both endpoints lie on a cached
//!   path at split points (`route_path`), the options are compatible
//!   (`route_cache::compatible`): the slice is the substructure of an
//!   optimal path, priced exactly — same cost as a fresh plan;
//! - splices: the start and/or the destination lie within
//!   `SPLICE_RADIUS` of a cached path. A bounded local walk search
//!   (walk steps only: no door, ride or toll, F2P bound, danger squares
//!   avoided, the planner's own step rule and step price) joins the
//!   start onto the path and leaves it toward the destination; the best
//!   join/leave pair minimizes the total. Epsilon rule: a splice of
//!   cost `total` is served only when `total - lower_bound <=
//!   splice_epsilon(total)`, `splice_epsilon(c) = max(SPLICE_EPSILON_MIN,
//!   c * SPLICE_EPSILON_PERCENT / 100)`, where `lower_bound` is a
//!   rigorous lower bound of the fresh plan's cost (triangle inequality
//!   over the cached path: d(O,T) >= d(X,Y) - d(X,O) - d(T,Y), maximised
//!   over the path split points X near O and Y near T, with d(X,Y) read
//!   off the cached cumulative costs and d(X,O), d(T,Y) bounded by two
//!   more local walks). The served cost is therefore PROVABLY within
//!   epsilon of a fresh plan; with diagonal steps the bound is usually
//!   tight (a start beside a straight path proves excess 0).
//!
//!   A blocked destination (tree, booth, NPC tile) splices onto the
//!   stand a cached adjusted plan for the same target ended on, when that
//!   stand sits on the nearest walkable ring (permissive view), open
//!   towards the target: the bound then subtracts the ring's spread (a
//!   fresh plan may pick another ring tile), so the served cost stays
//!   provably within epsilon too. Its served plan is adjusted like the
//!   fresh one (`adjusted_dest`, `dest_blocker`).
//!
//!   Only candidates whose stretch between the two ends costs at least
//!   `SPLICE_MIN_COST` are tried: a shorter fresh plan costs about what
//!   the four leg searches do (a short walk around a house still pays).

use std::sync::Arc;

use crate::plan::{NavOptions, Plan, Segment, Tile};
use crate::route_cache::{compatible, PlanEntry, ReuseKind};
use crate::route_patch::{LocalDist, Patch};
use crate::route_path::{self, push_walk};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Local walk reach around the start and the destination (Chebyshev).
pub const SPLICE_RADIUS: i32 = 8;
/// Costliest local leg searched (cost units, beyond the radius bound).
pub const SPLICE_LEG_HORIZON: u32 = 2 * SPLICE_RADIUS as u32;
/// Cheapest cached stretch worth a splice attempt: below it a fresh
/// plan costs about what the four leg searches do.
pub const SPLICE_MIN_COST: u32 = 32;
/// Epsilon floor (tiles) and share of the total route cost.
pub const SPLICE_EPSILON_MIN: u32 = 4;
pub const SPLICE_EPSILON_PERCENT: u32 = 6;

/// Largest excess a splice may carry over a fresh plan of cost `total`.
pub fn splice_epsilon(total: u32) -> u32 {
    (total * SPLICE_EPSILON_PERCENT / 100).max(SPLICE_EPSILON_MIN)
}

/// Best reuse of the candidates for `from -> dest`, or None (compute).
/// Returns the candidate id (LRU refresh), the reuse kind and the plan.
pub(crate) fn reuse(
    world: &StaticWorld,
    tables: &Tables,
    candidates: &[(u64, Arc<PlanEntry>)],
    from: Tile,
    dest: Tile,
    options: &NavOptions,
) -> Option<(u64, ReuseKind, Plan)> {
    // Exact sub-paths first: no search at all.
    for (id, entry) in candidates {
        let info = &entry.info;
        let start = if entry.from == from { Some(0) } else { info.split_index_of(from, 1) };
        let Some(start) = start else { continue };
        let end = if entry.dest == dest && entry.plan.adjusted_dest.is_none() && start < info.last() {
            Some(info.last())
        } else {
            info.split_index_of(dest, start + 1)
        };
        let Some(end) = end else { continue };
        if !compatible(entry, options, start, end) {
            continue;
        }
        let Some(cost) = info.cost_between(start, end) else { continue };
        let kind = match (start == 0, end == info.last()) {
            (true, true) => ReuseKind::Dominated,
            (true, false) => ReuseKind::Prefix,
            (false, true) => ReuseKind::Suffix,
            (false, false) => ReuseKind::Infix,
        };
        let segments = route_path::slice(&entry.plan, start, end);
        return Some((*id, kind, Plan { segments, cost, adjusted_dest: None, dest_blocker: None }));
    }
    splice(world, tables, candidates, from, dest, options)
}

/// Rigorous lower bound on a fresh plan's cost (module doc): for path
/// split points X near the start and Y near the destination,
/// d(start, dest) >= d(X, Y) - d(X, start) - d(dest, Y), with d(X, Y)
/// >= cum[Y] - cum[X] (the cached path is optimal and the request graph
/// is a subgraph of the entry's) and the walk-only local distances as
/// upper bounds of d(X, start) and d(dest, Y). Both maxima are taken
/// independently: X past Y only weakens the bound, never breaks it.
fn lower_bound(info: &route_path::PathInfo, cumulative: &[u32], join_back: &LocalDist, leave_back: &LocalDist) -> u32 {
    let mut best_join: Option<i64> = None;
    let mut best_leave: Option<i64> = None;
    for (index, tile) in info.path.iter().enumerate() {
        if !(info.split[index] || index == 0) {
            continue;
        }
        if let Some(back) = join_back.get(*tile) {
            let value = -i64::from(cumulative[index]) - i64::from(back);
            best_join = Some(best_join.map_or(value, |known| known.max(value)));
        }
        if let Some(back) = leave_back.get(*tile) {
            let value = i64::from(cumulative[index]) - i64::from(back);
            best_leave = Some(best_leave.map_or(value, |known| known.max(value)));
        }
    }
    match (best_join, best_leave) {
        (Some(join), Some(leave)) => (join + leave).clamp(0, i64::from(u32::MAX)) as u32,
        _ => 0,
    }
}

fn near(left: Tile, right: Tile) -> bool {
    left.level == right.level && (left.x - right.x).abs() <= SPLICE_RADIUS && (left.z - right.z).abs() <= SPLICE_RADIUS
}

/// Stand-in for a blocked destination (tree, booth, NPC tile): a cached
/// adjusted plan for the same target ended on `stand`, `stand` sits on
/// the nearest ring around the target holding any tile walkable in the
/// most permissive view (every door swung), open towards the target,
/// door-free. The ring tiles are a superset of every tile a fresh plan
/// could end on (they all share the best rank prefix or worse), so the
/// lower bound only loses their spread around `stand`.
struct BlockedStand {
    stand: Tile,
    ring: Vec<Tile>,
    blocker: Option<crate::plan::Blocker>,
}

fn blocked_stand(world: &StaticWorld, tables: &Tables, candidates: &[(u64, Arc<PlanEntry>)], dest: Tile, options: &NavOptions) -> Option<BlockedStand> {
    let (stand, blocker) = candidates.iter().find_map(|(_, entry)| {
        (entry.dest == dest).then_some(())?;
        entry.plan.adjusted_dest.map(|stand| (stand, entry.plan.dest_blocker.clone()))
    })?;
    if world.door_at(stand.x, stand.z, stand.level).is_some() || world.door_at(dest.x, dest.z, dest.level).is_some() {
        return None;
    }
    for ring in 1..=SPLICE_RADIUS {
        let mut tiles = Vec::new();
        for step_x in -ring..=ring {
            for step_z in -ring..=ring {
                if step_x.abs() != ring && step_z.abs() != ring {
                    continue;
                }
                let tile = Tile { x: dest.x + step_x, z: dest.z + step_z, level: dest.level };
                if options.f2p_only && !world.in_f2p(tile.x, tile.z) {
                    continue;
                }
                if crate::collision::tile_walkable(crate::pocket::permissive_flags(world, tables, tile)) {
                    tiles.push(tile);
                }
            }
        }
        if tiles.is_empty() {
            continue;
        }
        let walled = crate::fallback::walled_off(&mut |tile| world.get(tile.x, tile.z, tile.level), stand, dest);
        if stand.level != dest.level || stand.chebyshev(dest) != ring || !tiles.contains(&stand) || walled {
            return None;
        }
        return Some(BlockedStand { stand, ring: tiles, blocker });
    }
    None
}

fn splice(
    world: &StaticWorld,
    tables: &Tables,
    candidates: &[(u64, Arc<PlanEntry>)],
    from: Tile,
    dest: Tile,
    options: &NavOptions,
) -> Option<(u64, ReuseKind, Plan)> {
    // Blocked destination: splice onto the stand a cached adjusted plan
    // for the same target chose (never a guess: none cached, no splice).
    let blocked = if crate::collision::tile_walkable(world.get(dest.x, dest.z, dest.level)) {
        None
    } else {
        Some(blocked_stand(world, tables, candidates, dest, options)?)
    };
    let requested = dest;
    let dest = blocked.as_ref().map_or(dest, |stand| stand.stand);
    // Join and leave tiles: split points near each end (the cached start
    // itself joins too; the cached end leaves when it is the very dest).
    let mut worth: Vec<(u64, Arc<PlanEntry>)> = Vec::new();
    for (id, entry) in candidates {
        let Some(cumulative) = entry.info.cumulative.as_ref() else { continue };
        let mut first_join: Option<usize> = None;
        let mut last_leave: Option<usize> = None;
        for (index, tile) in entry.info.path.iter().enumerate() {
            if (entry.info.split[index] || index == 0) && near(*tile, from) && first_join.is_none() {
                first_join = Some(index);
            }
            if entry.info.split[index] && near(*tile, dest) {
                last_leave = Some(index);
            }
        }
        let (Some(join), Some(leave)) = (first_join, last_leave) else { continue };
        if leave <= join || cumulative[leave] - cumulative[join] < SPLICE_MIN_COST {
            continue;
        }
        worth.push((*id, Arc::clone(entry)));
    }
    if worth.is_empty() {
        return None;
    }
    let candidates = &worth;
    // Legs (start -> join, leave -> dest) and the opposite walks the
    // lower bound reads (join -> start, dest -> leave).
    let start_patch = Patch::read(world, options, from, SPLICE_RADIUS);
    let forward = start_patch.search(false, SPLICE_LEG_HORIZON);
    let join_back = start_patch.search(true, SPLICE_LEG_HORIZON);
    let dest_patch = Patch::read(world, options, dest, SPLICE_RADIUS);
    let backward = dest_patch.search(true, SPLICE_LEG_HORIZON);
    let leave_back = dest_patch.search(false, SPLICE_LEG_HORIZON);
    // A blocked target: a fresh plan may end on any ring tile s, and
    // d(start, s) >= d(start, stand) - d(s, stand): the bound loses the
    // ring's spread (every ring tile must walk to the stand locally).
    let spread = match &blocked {
        None => 0,
        Some(stand) => {
            let mut spread = 0;
            for tile in &stand.ring {
                spread = spread.max(backward.get(*tile)?);
            }
            spread
        }
    };

    let mut best: Option<(u32, u32, u64, usize, usize, Arc<PlanEntry>)> = None;
    for (id, entry) in candidates {
        let info = &entry.info;
        let Some(cumulative) = info.cumulative.as_ref() else { continue };
        // Join: minimize leg + cost of the rest from there; on a tie the
        // shorter leg (the epsilon bound reads the legs).
        let mut join: Option<((i64, u32), usize)> = None;
        for (index, tile) in info.path.iter().enumerate() {
            if !(info.split[index] || index == 0) || index == info.last() {
                continue;
            }
            if let Some(leg) = forward.get(*tile) {
                let score = (i64::from(leg) - i64::from(cumulative[index]), leg);
                if join.is_none_or(|(known, _)| score < known) {
                    join = Some((score, index));
                }
            }
        }
        let Some((_, start)) = join else { continue };
        let mut leave: Option<((i64, u32), usize)> = None;
        for index in start + 1..info.path.len() {
            if !info.split[index] {
                continue;
            }
            if let Some(leg) = backward.get(info.path[index]) {
                let score = (i64::from(leg) + i64::from(cumulative[index]), leg);
                if leave.is_none_or(|(known, _)| score < known) {
                    leave = Some((score, index));
                }
            }
        }
        let Some((_, end)) = leave else { continue };
        if !compatible(entry, options, start, end) {
            continue;
        }
        let (Some(join_leg), Some(leave_leg)) = (forward.get(info.path[start]), backward.get(info.path[end])) else { continue };
        let legs = join_leg + leave_leg;
        let total = legs + cumulative[end] - cumulative[start];
        let lower = lower_bound(info, cumulative, &join_back, &leave_back).saturating_sub(spread);
        if (legs > 0 || spread > 0) && total.saturating_sub(lower) > splice_epsilon(total) {
            continue;
        }
        if best.as_ref().is_none_or(|(known_total, known_legs, ..)| (total, legs) < (*known_total, *known_legs)) {
            best = Some((total, legs, *id, start, end, Arc::clone(entry)));
        }
    }
    let (total, legs, id, start, end, entry) = best?;
    let mut segments: Vec<Segment> = Vec::new();
    push_walk(&mut segments, forward.leg(entry.info.path[start]));
    for segment in route_path::slice(&entry.plan, start, end) {
        match segment {
            Segment::Walk { tiles } => push_walk(&mut segments, tiles),
            other => segments.push(other),
        }
    }
    // The reverse search walks dest-ward from the leave tile: dest last.
    push_walk(&mut segments, backward.leg(entry.info.path[end]));
    let kind = if legs == 0 && blocked.is_none() { ReuseKind::Infix } else { ReuseKind::Splice };
    let (adjusted_dest, dest_blocker) = match blocked {
        Some(stand) if stand.stand != requested => (Some(stand.stand), stand.blocker),
        _ => (None, None),
    };
    Some((id, kind, Plan { segments, cost: total, adjusted_dest, dest_blocker }))
}
