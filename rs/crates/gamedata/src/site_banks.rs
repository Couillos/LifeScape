//! The bank serving each site, by route cost (`gamedata/data/
//! site_banks.json`, tour 3 point 2).
//!
//! The bank of a site was the nearest booth as the crow flies: the
//! Lumbridge swamp mine banked at Al Kharid, 27 tiles away on the map and
//! 467 walked around the river, when Draynor is 170 (walk timeouts 24/h,
//! 38 approach NoRoute/h, tour 2 N3). The table prices, offline with the
//! planner's own costs (`lifescape_nav::site_banks`: doors, stairs,
//! rides, F2P, no paid edge), every site of `catalogue()` against the
//! seven F2P banks and keeps the cheapest route. Every bank choice reads
//! it: `danger::site_bank` (the combat requirement of a site and the
//! planner's site table), the scripts' spot pickers and serving bank.
//! Recomputed by `rs/crates/nav/tests/site_banks.rs`
//! (`LIFESCAPE_WRITE_SITE_BANKS=1` writes it): never drifts from the map.
//!
//! A tile outside the catalogue (a fishing spot that moved, a station
//! next to a tabled one) reads the nearest tabled site of its working
//! area (same level, `AREA_RADIUS`); none: no bank, typed by the caller.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/site_banks.json");

/// Radius of a working area (the planner's `CROWD_RADIUS`): an untabled
/// tile reads the nearest tabled site within it.
pub const AREA_RADIUS: i64 = 15;
/// Farthest bank (Chebyshev) a site is priced against: no route is
/// shorter than its Chebyshev distance, so a bank past it can never sit
/// within `danger::BANK_ROUTE_LIMIT`.
pub const PRICED_RADIUS: i64 = 300;

/// The bank serving a site and its route cost (tiles, doors and rides
/// priced like a plan).
#[derive(Debug, Clone)]
pub struct ServedBank {
    pub bank: crate::banks::Bank,
    pub name: String,
    pub cost: i64,
}

type Key = (i64, i64, i64);

struct Table {
    /// Site -> (bank index, cost); `None` for a site no bank route serves.
    rows: HashMap<Key, Option<(usize, i64)>>,
    banks: Vec<(String, Key)>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Table { rows: HashMap::new(), banks: Vec::new() };
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        for bank in doc.get("banks").as_arr().map(Vec::as_slice).unwrap_or(&[]) {
            let booth = bank.get("booth").as_arr().map(|cells| cells.iter().filter_map(|cell| cell.as_i64()).collect::<Vec<_>>()).unwrap_or_default();
            let (Some(name), [x, z, level]) = (bank.get("name").as_str(), booth.as_slice()) else { continue };
            out.banks.push((name.to_string(), (*x, *z, *level)));
        }
        for row in doc.get("sites").as_arr().map(Vec::as_slice).unwrap_or(&[]) {
            let cells: Vec<i64> = row.as_arr().map(|cells| cells.iter().filter_map(|cell| cell.as_i64()).collect()).unwrap_or_default();
            match cells.as_slice() {
                [x, z, level] => {
                    out.rows.insert((*x, *z, *level), None);
                }
                [x, z, level, bank, cost] if (*bank as usize) < out.banks.len() => {
                    out.rows.insert((*x, *z, *level), Some((*bank as usize, *cost)));
                }
                _ => {}
            }
        }
        out
    })
}

fn served(row: Option<(usize, i64)>) -> Option<ServedBank> {
    let (index, cost) = row?;
    let (name, (x, z, level)) = table().banks.get(index)?.clone();
    let bank = crate::banks::all_banks().into_iter().find(|bank| (bank.x, bank.z, bank.level) == (x, z, level))?;
    Some(ServedBank { bank, name, cost })
}

/// The bank serving the site at (`x`, `z`, `level`) and its route cost.
/// A Karamja site reads from the Port Sarim dock its boat lands on
/// (`access::mainland_anchor`). An untabled tile reads the nearest served
/// site of its working area (a sealed neighbour, a banker behind its
/// counter, never decides for it); `None` when no route serves it.
pub fn serving(x: i64, z: i64, level: i64) -> Option<ServedBank> {
    let key = crate::access::mainland_anchor(x, z, level);
    let rows = &table().rows;
    if let Some(row) = rows.get(&key) {
        return served(*row);
    }
    let near = rows
        .iter()
        .filter(|(site, row)| row.is_some() && site.2 == key.2 && (site.0 - key.0).abs().max((site.1 - key.1).abs()) <= AREA_RADIUS)
        .min_by_key(|(site, _)| ((site.0 - key.0).abs().max((site.1 - key.1).abs()), **site))?;
    served(*near.1)
}

/// Number of tabled sites (the dump parsed).
pub fn count() -> usize {
    table().rows.len()
}

/// Every site the table prices: the work sites of every gathering and
/// fight method (`danger::sites`), combat entrances, stations, F2P NPC
/// and ground-item spawns, gated areas' doors, and the ground tile under
/// any upper-floor one; Karamja anchored to its dock, sorted, unique.
pub fn catalogue() -> Vec<(i64, i64, i64)> {
    let free = |x: i64, z: i64| crate::free_zones::is_free(x, z);
    let mut out: Vec<(i64, i64, i64)> = Vec::new();
    let mut resources: Vec<(&str, String)> = Vec::new();
    for tier in crate::fishing::all_fish_tiers() {
        resources.extend(tier.products.iter().map(|product| ("fishing", product.to_string())));
    }
    resources.extend(crate::rocks::all_rock_tiers().into_iter().map(|tier| ("mining", tier.product.to_string())));
    resources.extend(crate::trees::all_tiers().into_iter().map(|tier| ("woodcut", tier.product.to_string())));
    resources.extend(crate::combat_spots::all_spots().into_iter().map(|spot| ("fight", spot.key.to_string())));
    resources.sort();
    resources.dedup();
    for (kind, resource) in &resources {
        out.extend(crate::danger::sites(kind, resource));
    }
    for spot in crate::combat_spots::all_spots() {
        if let Some(entrance) = spot.entrance {
            out.push((entrance.surface.0, entrance.surface.1, 0));
        }
    }
    out.extend(crate::stations::all_stations().into_iter().filter(|station| free(station.x, station.z)).map(|station| (station.x, station.z, station.level)));
    out.extend(crate::spawns::all_npc_spawns(None).into_iter().filter(|spawn| !spawn.members && free(spawn.x, spawn.z)).map(|spawn| (spawn.x, spawn.z, spawn.level)));
    out.extend(crate::spawns::all_obj_spawns(None).into_iter().filter(|spawn| !spawn.members && free(spawn.x, spawn.z)).map(|spawn| (spawn.x, spawn.z, spawn.level)));
    out.extend(crate::access::area_doors());
    let upper: Vec<(i64, i64, i64)> = out.iter().filter(|site| site.2 > 0).map(|site| (site.0, site.1, 0)).collect();
    out.extend(upper);
    let mut out: Vec<(i64, i64, i64)> = out.into_iter().map(|(x, z, level)| crate::access::mainland_anchor(x, z, level)).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::{count, serving};

    /// Tour 2 N3 / tour 3 point 2: the swamp mine (3244,3165) banks at
    /// Draynor (route 170), never at Al Kharid (27 tiles on the map, 467
    /// walked around the river).
    #[test]
    fn the_swamp_mine_banks_at_draynor() {
        assert!(count() > 1000, "dump parsed: {}", count());
        let served = serving(3244, 3165, 0).expect("a bank serves the swamp mine");
        assert_eq!(served.name, "draynor", "{served:?}");
        assert!(served.cost < 300, "{served:?}");
        let near = serving(3243, 3166, 0).expect("its neighbour rock reads the same area");
        assert_eq!(near.name, "draynor");
    }
}
