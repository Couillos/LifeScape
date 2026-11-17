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
                PocketVerdict::Closed { edge_in, .. } => edge_in,
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

