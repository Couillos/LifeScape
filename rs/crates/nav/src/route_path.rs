#![forbid(unsafe_code)]
//! Path analysis for the route cache (CPU 2026-11-14).
//!
//! A cached plan is reused for sub-paths and splices only through this
//! analysis, computed once at insert:
//! - the walking-order tile path (`from` first) and what reached each
//!   tile (walk step, door crossing, ride/custom landing);
//! - the exact planner cost at every path index, re-priced step by step
//!   with the planner's own formula (`plan.rs` `dijkstra`): walk = 1 +
//!   danger + door-leaf extra, door = `DOOR_COST` per Dijkstra edge,
//!   ride = `TRANSITION_COST` (+ `UNVERIFIED_PENALTY`), custom =
//!   `TRANSITION_COST + UNVERIFIED_PENALTY`. When the re-priced total
//!   differs from the plan's cost, sub-path reuse is disabled for that
//!   entry (whole-path hits only): a sub-path is never mispriced;
//! - split points: tiles reached by a walk step (the planner's normal
//!   search state, never the FRESH door-click state) that hold no door.
//!   A fresh plan from or to such a tile starts or ends in that same
//!   state, so a sub-path between split points is exactly the
//!   substructure of the cached optimal path;
//! - the option-dependent edges crossed: every door / ride / custom with
//!   its `EdgeId` (banned edges), requirement tags and paid flag (toll
//!   gate or fare).

use crate::collision::leaf_step_extra;
use crate::plan::{self, DangerMap, DangerZone, EdgeId, Plan, Segment, Tile};
use crate::static_world::StaticWorld;
use crate::tables::{Tables, Verdict};

/// What reached a path tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Arrival {
    Origin,
    Walk,
    Door,
    Ride,
}

/// One door / ride / custom crossed, at the path index it lands on.
#[derive(Clone, Debug)]
pub(crate) struct EdgeUse {
    pub at: usize,
    pub id: EdgeId,
    /// Tags the edge needs (door requirement rows, custom `requires`).
    pub requires: Vec<String>,
    /// Toll gate: only `allow_paid` requests may cross it (a fare edge's
    /// coins are a `requires` tag instead).
    pub paid: bool,
}

/// Everything the cache needs to serve a sub-path of one plan.
#[derive(Clone, Debug)]
pub(crate) struct PathInfo {
    pub path: Vec<Tile>,
    pub arrival: Vec<Arrival>,
    /// Exact planner cost at each index; `None` when the re-pricing
    /// disagreed with the plan's total (sub-paths disabled).
    pub cumulative: Option<Vec<u32>>,
    pub split: Vec<bool>,
    pub edges: Vec<EdgeUse>,
}

impl PathInfo {
    pub fn last(&self) -> usize {
        self.path.len().saturating_sub(1)
    }

    /// Cost between two path indices (`None` without exact pricing).
    pub fn cost_between(&self, start: usize, end: usize) -> Option<u32> {
        let cumulative = self.cumulative.as_ref()?;
        Some(cumulative.get(end)?.saturating_sub(*cumulative.get(start)?))
    }

    /// First split-point index holding `tile` at or after `from_index`.
    pub fn split_index_of(&self, tile: Tile, from_index: usize) -> Option<usize> {
        (from_index..self.path.len()).find(|index| self.path[*index] == tile && self.split[*index])
    }

    /// Rough RAM of the entry (cache byte bound).
    pub fn bytes(&self) -> usize {
        self.path.len() * 40 + self.edges.len() * 64 + 256
    }
}

/// Walking-order tile path of a plan, `from` first, with arrivals.
pub(crate) fn plan_path(from: Tile, plan: &Plan) -> (Vec<Tile>, Vec<Arrival>) {
    let mut path = vec![from];
    let mut arrival = vec![Arrival::Origin];
    for segment in &plan.segments {
        match segment {
            Segment::Walk { tiles } => {
                path.extend(tiles.iter().copied());
                arrival.extend(tiles.iter().map(|_| Arrival::Walk));
            }
            Segment::OpenDoor { cross_to, .. } => {
                path.push(*cross_to);
                arrival.push(Arrival::Door);
            }
            Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => {
                path.push(*expect);
                arrival.push(Arrival::Ride);
            }
        }
    }
    (path, arrival)
}

/// Full analysis of one computed plan under the options it was planned
/// with (`danger` prices the walk steps exactly like the search did).
pub(crate) fn analyze(world: &StaticWorld, tables: &Tables, from: Tile, plan: &Plan, danger: &[DangerZone]) -> PathInfo {
    let (path, arrival) = plan_path(from, plan);
    let danger_map = DangerMap::new(danger);
    let mut cumulative = Vec::with_capacity(path.len());
    cumulative.push(0u32);
    let mut edges = Vec::new();
    let mut priced_ok = true;
    let mut index = 0usize;
    let mut running = 0u32;
    for segment in &plan.segments {
        match segment {
            Segment::Walk { tiles } => {
                for tile in tiles {
                    let here = path[index];
                    let (step_x, step_z) = (tile.x - here.x, tile.z - here.z);
                    if step_x.abs() > 1 || step_z.abs() > 1 || tile.level != here.level {
                        priced_ok = false;
                    }
                    running += 1
                        + danger_map.extra(*tile)
                        + leaf_step_extra(world.get(here.x, here.z, here.level), world.get(tile.x, tile.z, tile.level), step_x, step_z);
                    index += 1;
                    cumulative.push(running);
                }
            }
            Segment::OpenDoor { door, stand, cross_to } => {
                let door_tile = Tile { x: door.x, z: door.z, level: door.level };
                let door_edges = u32::from(*stand != door_tile) + u32::from(*cross_to != door_tile);
                running += door_edges * plan::DOOR_COST;
                index += 1;
                cumulative.push(running);
                let requires = tables.missing(door.loc, door_tile, &std::collections::HashSet::new());
                edges.push(EdgeUse {
                    at: index,
                    id: EdgeId::Door { loc: door.loc, x: door.x, z: door.z, level: door.level },
                    requires,
                    paid: door.toll || world.is_toll(door.x, door.z, door.level),
                });
            }
            Segment::Ride { transition, .. } => {
                let verified = tables.verdict(transition.loc, transition.from).is_some_and(Verdict::verified);
                running += plan::TRANSITION_COST + if verified { 0 } else { plan::UNVERIFIED_PENALTY };
                index += 1;
                cumulative.push(running);
                edges.push(EdgeUse { at: index, id: EdgeId::Ride(transition.name.clone()), requires: Vec::new(), paid: false });
            }
            Segment::Custom { edge, .. } => {
                running += plan::TRANSITION_COST + plan::UNVERIFIED_PENALTY;
                index += 1;
                cumulative.push(running);
                let row = tables.customs.iter().find(|row| row.name == edge.name);
                edges.push(EdgeUse {
                    at: index,
                    id: EdgeId::Custom(edge.name.clone()),
                    requires: row.map(|row| row.requires.clone()).unwrap_or_default(),
                    // The boat's fare rides in `requires` (coins tag): no
                    // `allow_paid` gate (user 2026-11-16, Karamja universal).
                    paid: false,
                });
                if row.is_none() {
                    priced_ok = false;
                }
            }
        }
    }
    let split: Vec<bool> = path
        .iter()
        .zip(arrival.iter())
        .map(|(tile, arrived)| {
            matches!(arrived, Arrival::Walk | Arrival::Origin) && world.door_at(tile.x, tile.z, tile.level).is_none()
        })
        .collect();
    let exact = priced_ok && running == plan.cost && cumulative.len() == path.len();
    PathInfo { path, arrival, cumulative: if exact { Some(cumulative) } else { None }, split, edges }
}

/// Segments covering path indices `(start, end]` of `plan`: whole
/// door/ride/custom segments landing inside, walk tiles cut to the range.
pub(crate) fn slice(plan: &Plan, start: usize, end: usize) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for segment in &plan.segments {
        match segment {
            Segment::Walk { tiles } => {
                let kept: Vec<Tile> = tiles
                    .iter()
                    .enumerate()
                    .filter(|(offset, _)| {
                        let index = at + 1 + offset;
                        index > start && index <= end
                    })
                    .map(|(_, tile)| *tile)
                    .collect();
                at += tiles.len();
                if !kept.is_empty() {
                    push_walk(&mut out, kept);
                }
            }
            other => {
                at += 1;
                if at > start && at <= end {
                    out.push(other.clone());
                }
            }
        }
        if at >= end {
            break;
        }
    }
    out
}

/// Appends walk tiles, merging into a trailing walk (one walk per run).
pub(crate) fn push_walk(segments: &mut Vec<Segment>, tiles: Vec<Tile>) {
    if tiles.is_empty() {
        return;
    }
    if let Some(Segment::Walk { tiles: last }) = segments.last_mut() {
        last.extend(tiles);
        return;
    }
    segments.push(Segment::Walk { tiles });
}

/// Every tile of `zone` inside some zone of `cover` (same level, square
/// containment): the conservative tile-set inclusion the danger rule uses.
/// Identical lists (one bot's unchanged arm) and squares present as-is
/// in `cover` answer without the pairwise scan (CPU 2026-11-14: ~180
/// live squares made it 32 k comparisons per candidate).
pub(crate) fn zones_covered(zones: &[DangerZone], cover: &[DangerZone]) -> bool {
    if zones == cover {
        return true;
    }
    let sorted_cover = cover.is_sorted();
    zones.iter().all(|zone| {
        if listed(cover, sorted_cover, zone) {
            return true;
        }
        let radius = zone.radius.clamp(0, 32);
        cover.iter().any(|outer| {
            let outer_radius = outer.radius.clamp(0, 32);
            outer.level == zone.level
                && outer.x - outer_radius <= zone.x - radius
                && outer.x + outer_radius >= zone.x + radius
                && outer.z - outer_radius <= zone.z - radius
                && outer.z + outer_radius >= zone.z + radius
        })
    })
}

/// `zone` listed as-is in `list` (binary search when sorted: the
/// service sorts every request's squares).
fn listed(list: &[DangerZone], sorted: bool, zone: &DangerZone) -> bool {
    if sorted {
        list.binary_search(zone).is_ok()
    } else {
        list.contains(zone)
    }
}

/// Squares of `zones` not listed as-is in `known`.
pub(crate) fn zones_missing(zones: &[DangerZone], known: &[DangerZone]) -> Vec<DangerZone> {
    if zones == known {
        return Vec::new();
    }
    let sorted_known = known.is_sorted();
    zones.iter().filter(|zone| !listed(known, sorted_known, zone)).copied().collect()
}

/// True when `tile` lies in one of `zones` (same clamp as `DangerMap`).
pub(crate) fn in_zones(zones: &[DangerZone], tile: Tile) -> bool {
    zones.iter().any(|zone| {
        let radius = zone.radius.clamp(0, 32);
        zone.level == tile.level && (tile.x - zone.x).abs() <= radius && (tile.z - zone.z).abs() <= radius
    })
}
