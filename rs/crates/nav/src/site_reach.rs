//! Work sites proven reachable (plan P6, 2026-11-17).
//!
//! A tree or rock is a work site only when a tile the bot can stand on
//! to work it is walkable, reaches the loc by the server's interaction
//! rule (`reach::reached`: walls and fences between stand and loc
//! count), and is not a sealed pocket: the same flood the planner
//! trusts for `Disconnected` (`pocket::verdict`, every openable door
//! swung, rides and custom edges counted). Live 2026-11-17: 288 NoRoute
//! dossiers on one river willow (3168,3350) behind
//! `inviswall_blockrange`, 387 more on trees and rocks behind garden
//! fences and the Al Kharid bank wall, each an exhausted search (3-11 s
//! CPU) and a "site not reached".
//!
//! The verdict is computed offline (`orchestrator dump-site-reach`) into
//! `gamedata/data/unreachable_sites.json`, which `gamedata::danger::sites`
//! reads; the golden test recomputes the dump so it never drifts from
//! the collision.
//!
//! Combat camps: `spawn_verdicts` (same pocket rule around an NPC spawn)
//! and, for the underground spawns, `route_verdicts` (a plan from the
//! surface and back): `gamedata/data/unreachable_spawns.json`, recomputed
//! by `rs/crates/nav/tests/fight_sites.rs`.

#![forbid(unsafe_code)]

use crate::collision::tile_walkable;
use crate::plan::{prepare_shared, NavOptions, SharedEdges, Tile};
use crate::pocket::{verdict, PocketVerdict};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Why a site is no work site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteVerdict {
    /// A stand reaches the loc and lies in the open world.
    Reachable,
    /// No loc `loc_id` on the tile: nothing to judge (kept).
    NoLoc,
    /// Every stand is unwalkable or cannot reach the loc (wall, fence,
    /// invisible blocker between them).
    NoStand,
    /// The stands that reach it are sealed pockets nothing enters.
    Sealed,
}

impl SiteVerdict {
    /// Typed reason written in the dump.
    pub fn reason(&self) -> &'static str {
        match self {
            SiteVerdict::Reachable => "reachable",
            SiteVerdict::NoLoc => "no_loc",
            SiteVerdict::NoStand => "no_stand",
            SiteVerdict::Sealed => "sealed",
        }
    }
}

/// Verdicts of many sites `(loc id, anchor)` against one F2P edge set
/// (`None` when the edges cannot be prepared).
pub fn site_verdicts(world: &StaticWorld, tables: &Tables, sites: &[(u16, Tile)]) -> Option<Vec<SiteVerdict>> {
    let options = NavOptions { f2p_only: true, ..NavOptions::default() };
    let shared = prepare_shared(world, tables, &options, &mut || true)?;
    Some(sites.iter().map(|(loc_id, origin)| site_verdict(world, tables, &shared, *loc_id, *origin)).collect())
}

/// Verdicts of many NPC spawn tiles (a combat camp's foes) against one
/// F2P edge set (`None` when the edges cannot be prepared). A spawn is a
/// fight site when the tile it stands on, or a walkable neighbour the bot
/// steps up to, lies in the open world (`pocket::verdict`, the planner's
/// own `Disconnected` proof). Live 2026-11-21: the caged lesser demon at
/// the top of the Wizards' Tower (3110,3157,2) was a camp's only site the
/// brain priced, `Disconnected` on every walk (25 failures in 6 h).
pub fn spawn_verdicts(world: &StaticWorld, tables: &Tables, spawns: &[Tile]) -> Option<Vec<SiteVerdict>> {
    let options = NavOptions { f2p_only: true, ..NavOptions::default() };
    let shared = prepare_shared(world, tables, &options, &mut || true)?;
    Some(spawns.iter().map(|spawn| spawn_verdict(world, tables, &shared, *spawn)).collect())
}

/// Verdict of one NPC spawn tile (`spawn_verdicts`).
fn spawn_verdict(world: &StaticWorld, tables: &Tables, shared: &SharedEdges, spawn: Tile) -> SiteVerdict {
    let mut get = |x: i32, z: i32| world.get(x, z, spawn.level);
    let spawn_walkable = tile_walkable(world.get(spawn.x, spawn.z, spawn.level));
    let mut stands = 0;
    for dx in -1..=1 {
        for dz in -1..=1 {
            let stand = Tile { x: spawn.x + dx, z: spawn.z + dz, level: spawn.level };
            if !tile_walkable(world.get(stand.x, stand.z, stand.level)) {
                continue;
            }
            // Bars, a wall or a fence between the stand and the foe: no
            // melee exchange from there (a walkable foe tile is stepped
            // onto by the server's own move rule).
            if stand != spawn && spawn_walkable && !crate::bfs::can_step(&mut get, stand.x, stand.z, spawn.x, spawn.z) {
                continue;
            }
            stands += 1;
            if matches!(verdict(world, tables, shared, stand), PocketVerdict::Open) {
                return SiteVerdict::Reachable;
            }
        }
    }
    if stands == 0 {
        SiteVerdict::NoStand
    } else {
        SiteVerdict::Sealed
    }
}

/// Chebyshev radius (same level) within which a tile borrows the route
/// verdict of an already judged one: when the planner walks between the
/// two both ways, they share the seed's answer.
const ROUTE_SHARE_RADIUS: i32 = 24;
/// Judged neighbours a tile tries before its own full verdict.
const ROUTE_SHARE_TRIES: usize = 2;
/// Wall budget of one walk between two neighbours: a cut walk only means
/// "no borrowed verdict", the tile is then judged against the seed.
const ROUTE_SHARE_BUDGET: std::time::Duration = std::time::Duration::from_millis(400);
/// Wall budget of one seed walk: generous, so a verdict never depends on
/// the machine's load (an exhaustive F2P miss takes ~2 s in release).
const ROUTE_SEED_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

/// Route verdicts of many tiles against one seed (`None` when the edges
/// cannot be prepared): true when the planner, under `options`, walks
/// `seed` → tile AND tile → `seed` (the bot gets there and back to its
/// bank). A sealed pocket says `Disconnected` at once, but a large region
/// no edge enters (an underground hall behind a slashable web, Elvarg's
/// lair behind the one-way secret door) passes `spawn_verdicts`: only a
/// plan proves the way in. A tile within `ROUTE_SHARE_RADIUS` of a judged
/// one that the planner joins both ways takes its verdict (a dungeon's
/// spawns are judged once, not one cross-map plan each); the verdicts are
/// the same either way.
pub fn route_verdicts(world: &StaticWorld, tables: &Tables, options: &NavOptions, seed: Tile, tiles: &[Tile]) -> Option<Vec<bool>> {
    let shared = prepare_shared(world, tables, options, &mut || true)?;
    let walks = |from: Tile, to: Tile, budget: std::time::Duration| {
        crate::plan::plan_budgeted(world, tables, from, to, options, &shared, &mut || true, budget).is_ok()
    };
    let mut verdicts: Vec<bool> = Vec::with_capacity(tiles.len());
    for (index, tile) in tiles.iter().enumerate() {
        let spread = |other: &Tile| (other.x - tile.x).abs().max((other.z - tile.z).abs());
        let mut near: Vec<usize> = (0..index).filter(|judged| tiles[*judged].level == tile.level && spread(&tiles[*judged]) <= ROUTE_SHARE_RADIUS).collect();
        near.sort_by_key(|judged| (spread(&tiles[*judged]), *judged));
        // Out first: a tile in a region no edge leaves fails at once.
        let borrowed = near
            .into_iter()
            .take(ROUTE_SHARE_TRIES)
            .find(|judged| walks(*tile, tiles[*judged], ROUTE_SHARE_BUDGET) && walks(tiles[*judged], *tile, ROUTE_SHARE_BUDGET))
            .map(|judged| verdicts[judged]);
        let verdict = borrowed.unwrap_or_else(|| walks(*tile, seed, ROUTE_SEED_BUDGET) && walks(seed, *tile, ROUTE_SEED_BUDGET));
        verdicts.push(verdict);
    }
    Some(verdicts)
}

/// Verdict for loc `loc_id` anchored at `origin`.
pub(crate) fn site_verdict(world: &StaticWorld, tables: &Tables, shared: &SharedEdges, loc_id: u16, origin: Tile) -> SiteVerdict {
    let Some((_, shape, angle)) = world.locs_at(origin.x, origin.z, origin.level).into_iter().find(|(id, _, _)| *id == loc_id) else {
        return SiteVerdict::NoLoc;
    };
    let (mut width, mut length) = (1, 1);
    if let Some(def) = world.loc_def(usize::from(loc_id)) {
        width = def.width.max(1);
        length = def.length.max(1);
    }
    // Footprint on the map: rotated a quarter turn, width spans z.
    let (span_x, span_z) = if angle % 2 == 1 { (length, width) } else { (width, length) };
    let level = origin.level;
    let get = |x: i32, z: i32, plane: i32| world.get(x, z, plane);
    let mut stands_reaching = 0;
    for x in origin.x - 1..=origin.x + span_x {
        for z in origin.z - 1..=origin.z + span_z {
            let inside = x >= origin.x && x < origin.x + span_x && z >= origin.z && z < origin.z + span_z;
            if inside || !tile_walkable(world.get(x, z, level)) {
                continue;
            }
            if !crate::reach::reached(&get, level, x, z, origin.x, origin.z, width, length, i32::from(angle), i32::from(shape), 0) {
                continue;
            }
            stands_reaching += 1;
            let open = match verdict(world, tables, shared, Tile { x, z, level }) {
                PocketVerdict::Open => true,
                PocketVerdict::Closed { .. } => false,
            };
            if open {
                return SiteVerdict::Reachable;
            }
        }
    }
    if stands_reaching == 0 {
        SiteVerdict::NoStand
    } else {
        SiteVerdict::Sealed
    }
}

