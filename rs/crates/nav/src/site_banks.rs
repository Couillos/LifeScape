//! The bank serving each work site, by route cost (tour 3, point 2).
//!
//! The bank of a site was the nearest booth as the crow flies
//! (`danger::site_bank`, the spot pickers' `nearest_bank_at`): the
//! Lumbridge swamp mine banked at Al Kharid, 27 tiles away on the map and
//! 467 walked around the river (Draynor: 170). Every site of the
//! catalogue now reads its bank from `gamedata/data/site_banks.json`,
//! computed here offline with the planner's own costs (`costs::
//! plan_costs`: doors, stairs, rides, F2P, no paid edge): the bank whose
//! route reaches the site's stand cheapest, within `limit`. One search
//! per bank prices every site at once (the bank's trip both ways rides
//! the same edges; a one-way edge is the only asymmetry). The golden test
//! (`tests/site_banks.rs`) recomputes the table so it never drifts from
//! the collision.

#![forbid(unsafe_code)]

use crate::collision::tile_walkable;
use crate::plan::{prepare_shared, NavOptions, Tile};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// The bank serving one site: index into the bank list and route cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Served {
    pub bank: usize,
    pub cost: u32,
}

/// Where a route to `site` ends: the site itself when walkable and open,
/// else its nearest walkable tile within two tiles (a rock, a tree, a
/// fishing spot on the water, a counter) that is no sealed pocket (a tile
/// boxed in by the rocks of its own cluster: Varrock east iron), `None`
/// when none is.
pub(crate) fn open_stand(world: &StaticWorld, tables: &Tables, shared: &crate::plan::SharedEdges, site: Tile) -> Option<Tile> {
    let open = |tile: Tile| match crate::pocket::verdict(world, tables, shared, tile) {
        crate::pocket::PocketVerdict::Open => true,
        crate::pocket::PocketVerdict::Closed { edge_in, .. } => edge_in,
    };
    let mut first: Option<Tile> = None;
    for tile in stand_candidates(world, site) {
        first.get_or_insert(tile);
        if open(tile) {
            return Some(tile);
        }
    }
    // Every candidate sealed: the nearest one (no route will reach it).
    first
}

/// Walkable tiles a site is reached from, nearest first: the site itself,
/// then its rings at one and two tiles (orthogonal first, fixed order).
fn stand_candidates(world: &StaticWorld, site: Tile) -> Vec<Tile> {
    let mut out: Vec<Tile> = Vec::new();
    if tile_walkable(world.get(site.x, site.z, site.level)) {
        out.push(site);
    }
    for radius in 1i32..=2 {
        let mut ring: Vec<Tile> = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs().max(dz.abs()) != radius {
                    continue;
                }
                let tile = Tile { x: site.x + dx, z: site.z + dz, level: site.level };
                if tile_walkable(world.get(tile.x, tile.z, tile.level)) {
                    ring.push(tile);
                }
            }
        }
        // Orthogonal neighbours first (a stand that works the loc), then
        // a fixed order: the table never depends on iteration order.
        ring.sort_by_key(|tile| ((tile.x - site.x).abs() + (tile.z - site.z).abs(), tile.x, tile.z));
        out.extend(ring);
    }
    out
}

/// One capped multi-target search per bank (in parallel) over the
/// `(site slot, stand)` targets; each slot keeps its cheapest route in
/// `out` (ties: the lower bank index). False when a search panicked.
fn inbound(
    world: &StaticWorld,
    tables: &Tables,
    (shared, memo, options): (&crate::plan::SharedEdges, &crate::pocket::PocketMemo, &NavOptions),
    banks: &[Tile],
    targets: &[(usize, Tile)],
    limit: i32,
    out: &mut [Option<Served>],
) -> bool {
    let cap = limit.max(0) as u32;
    let mut complete = true;
    std::thread::scope(|scope| {
        let workers: Vec<_> = banks
            .iter()
            .enumerate()
            .map(|(index, bank)| {
                scope.spawn(move || {
                    let near: Vec<(usize, Tile)> = targets.iter().copied().filter(|(_, tile)| (tile.x - bank.x).abs().max((tile.z - bank.z).abs()) <= limit).collect();
                    let dests: Vec<Tile> = near.iter().map(|(_, tile)| *tile).collect();
                    let costs = crate::costs::plan_costs_capped(world, tables, *bank, &dests, options, shared, Some(memo), cap);
                    (index, near.into_iter().map(|(slot, _)| slot).zip(costs).collect::<Vec<_>>())
                })
            })
            .collect();
        for worker in workers {
            let Ok((index, priced)) = worker.join() else {
                complete = false;
                continue;
            };
            for (slot, cost) in priced {
                let Some(cost) = cost else { continue };
                if out[slot].is_none_or(|known| (cost, index) < (known.cost, known.bank)) {
                    out[slot] = Some(Served { bank: index, cost });
                }
            }
        }
    });
    complete
}

/// Cheapest bank of `banks` (their stand tiles) for every site of
/// `sites`: one capped multi-target search per bank (`limit`: route cost,
/// and Chebyshev reach of the candidates, a lower bound of any route),
/// the cheapest route wins (ties: the lower bank index). `None` where no
/// bank route within `limit` reaches the site; `None` overall when the
/// edge set cannot be prepared. Banks are searched in parallel.
pub fn serving_banks(world: &StaticWorld, tables: &Tables, sites: &[Tile], banks: &[Tile], limit: i32) -> Option<Vec<Option<Served>>> {
    let options = NavOptions { f2p_only: true, ..NavOptions::default() };
    let shared = prepare_shared(world, tables, &options, &mut || true)?;
    // Pocket verdicts of the stands, flooded once for every bank.
    let memo = crate::pocket::PocketMemo::default();
    let stands: Vec<Option<Tile>> = sites.iter().map(|site| open_stand(world, tables, &shared, *site)).collect();
    let cap = limit.max(0) as u32;
    let mut out: Vec<Option<Served>> = vec![None; sites.len()];
    // Inbound from every bank: each site's open stand first; a site still
    // unserved then tries every other tile it is reached from (the ladder
    // hut of the Mining Guild: its first open tile is walled off).
    let first: Vec<(usize, Tile)> = stands.iter().enumerate().filter_map(|(slot, stand)| stand.map(|tile| (slot, tile))).collect();
    let mut complete = inbound(world, tables, (&shared, &memo, &options), banks, &first, limit, &mut out);
    let mut others: Vec<(usize, Tile)> = Vec::new();
    for (slot, site) in sites.iter().enumerate().filter(|(slot, _)| out[*slot].is_none()) {
        others.extend(stand_candidates(world, *site).into_iter().filter(|tile| Some(*tile) != stands[slot]).map(|tile| (slot, tile)));
    }
    complete &= inbound(world, tables, (&shared, &memo, &options), banks, &others, limit, &mut out);
    if !complete {
        return None;
    }
    // A site no bank reaches inbound (a guild door the bank side cannot
    // open, a one-way door) is priced outbound: the bot already works
    // there and walks out to bank (live: the crafting guild's pottery
    // oven, 134 tiles to Falador east outbound, no route inbound).
    let reach = |stand: &Tile, bank: &Tile| (bank.x - stand.x).abs().max((bank.z - stand.z).abs()) <= limit;
    let missing: Vec<(usize, Tile)> = stands
        .iter()
        .enumerate()
        .filter(|(slot, stand)| out[*slot].is_none() && stand.is_some_and(|tile| banks.iter().any(|bank| reach(&tile, bank))))
        .filter_map(|(slot, stand)| stand.map(|tile| (slot, tile)))
        .collect();
    let threads = std::thread::available_parallelism().map(|count| count.get()).unwrap_or(4).max(1);
    std::thread::scope(|scope| {
        let (shared, memo, options, missing) = (&shared, &memo, &options, &missing);
        let workers: Vec<_> = (0..threads)
            .map(|worker| {
                scope.spawn(move || {
                    let mut found: Vec<(usize, Served)> = Vec::new();
                    for (slot, stand) in missing.iter().skip(worker).step_by(threads) {
                        let near: Vec<(usize, Tile)> = banks.iter().copied().enumerate().filter(|(_, bank)| reach(stand, bank)).collect();
                        let targets: Vec<Tile> = near.iter().map(|(_, bank)| *bank).collect();
                        let costs = crate::costs::plan_costs_capped(world, tables, *stand, &targets, options, shared, Some(memo), cap);
                        let best = near.iter().zip(costs).filter_map(|((index, _), cost)| cost.map(|cost| Served { bank: *index, cost })).min_by_key(|served| (served.cost, served.bank));
                        if let Some(best) = best {
                            found.push((*slot, best));
                        }
                    }
                    found
                })
            })
            .collect();
        for worker in workers {
            match worker.join() {
                Ok(found) => {
                    for (slot, served) in found {
                        out[slot] = Some(served);
                    }
                }
                Err(_) => complete = false,
            }
        }
    });
    complete.then_some(out)
}
