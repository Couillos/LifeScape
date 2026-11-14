#![forbid(unsafe_code)]
//! Worker-side cache of STATIC service answers, pinned by snapshot
//! version (CPU 2026-11-14).
//!
//! `door_defs`, `tiles_flags`, `in_f2p`, `static_locs_near`, `toll_fence`
//! and `walk_costs` are pure functions of (snapshot version, request):
//! the same request under the same version always gets the same answer.
//! Measured on the live fleet (85 min, ~26 bots) they were ~40k round
//! trips, mostly repeats (npc rings, fence lines, booths, re-ranked
//! windows). This cache answers repeats with no round trip:
//! - per-key maps for `door_defs` (loc id), `tiles_flags` and `in_f2p`
//!   (tile): a batch is served only when EVERY member is known,
//!   otherwise the whole batch goes out exactly as before;
//! - covering lists for `toll_fence` and `static_locs_near`
//!   (`lifescape_nav::region_lists`): any query whose diamond fits in a
//!   served list is answered by filtering it, same order and ties;
//! - whole-batch memo for `walk_costs` (source, target set, options).
//!
//! Invalidation: every entry belongs to `version`; a lookup or insert
//! under another version clears everything first (reload-nav bumps it,
//! the worker learns it from any reply and the next lookup drops the
//! old answers). No version pinned (bare bots) means no caching.
//! Bounded: each map is cleared when full (a cache, never truth).

use std::collections::HashMap;

use lifescape_nav::plan::{NavOptions, Tile};
use lifescape_nav::region_lists::{LocsList, TollList};
use lifescape_nav::service::PlanKey;
use lifescape_nav::service_queries::{DoorDef, StaticLoc, TileFlags};

/// Tiles kept per map (flags, F2P) before a clear: ~1.5 MB worst case.
pub const TILE_CAP: usize = 32_768;
/// Loc ids kept before a clear.
pub const DEFS_CAP: usize = 4_096;
/// Covering lists kept per kind (oldest dropped first).
pub const LISTS_CAP: usize = 8;
/// Walk-cost batches kept before a clear.
pub const COSTS_CAP: usize = 64;
/// Extra radius asked with a `toll_fence` list: the bot may wander this
/// far (Manhattan) from the asking tile and still be answered locally.
pub const TOLL_LIST_MARGIN: i32 = 48;

/// Hit/miss counters per cached op (worker `cost_stats`-style gauge).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaticCacheStats {
    pub hits: u64,
    pub misses: u64,
}

#[derive(Default)]
pub(crate) struct StaticCache {
    version: String,
    defs: HashMap<u16, DoorDef>,
    tiles: HashMap<(i32, i32, i32), TileFlags>,
    f2p: HashMap<(i32, i32, i32), bool>,
    tolls: Vec<TollList>,
    locs: Vec<LocsList>,
    costs: HashMap<(PlanKey, Vec<Tile>), Vec<Option<u32>>>,
    pub stats: StaticCacheStats,
}

/// Sorted distinct targets: the memo key and its value order.
pub(crate) fn costs_set(dests: &[Tile]) -> Vec<Tile> {
    let mut set = dests.to_vec();
    set.sort_by_key(|tile| (tile.level, tile.x, tile.z));
    set.dedup();
    set
}

fn push_bounded<T>(lists: &mut Vec<T>, list: T) {
    if lists.len() >= LISTS_CAP {
        lists.remove(0);
    }
    lists.push(list);
}

impl StaticCache {
    /// Adopt `version` (empty: caching off). A different version drops
    /// every entry: answers never outlive their snapshot.
    fn pin(&mut self, version: &str) -> bool {
        if version.is_empty() {
            return false;
        }
        if self.version != version {
            let stats = self.stats;
            *self = StaticCache { version: version.to_string(), stats, ..StaticCache::default() };
        }
        true
    }

    fn count(&mut self, hit: bool) {
        if hit {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
        }
    }

    pub fn door_defs(&mut self, version: &str, locs: &[u16]) -> Option<Vec<DoorDef>> {
        if !self.pin(version) {
            return None;
        }
        let found: Option<Vec<DoorDef>> = locs.iter().map(|loc| self.defs.get(loc).cloned()).collect();
        self.count(found.is_some());
        found
    }

    pub fn store_door_defs(&mut self, version: &str, defs: &[DoorDef]) {
        if !self.pin(version) {
            return;
        }
        if self.defs.len() + defs.len() > DEFS_CAP {
            self.defs.clear();
        }
        for def in defs {
            self.defs.insert(def.loc, def.clone());
        }
    }

    pub fn tiles_flags(&mut self, version: &str, tiles: &[(i32, i32, i32)]) -> Option<Vec<TileFlags>> {
        if !self.pin(version) {
            return None;
        }
        let found: Option<Vec<TileFlags>> = tiles.iter().map(|tile| self.tiles.get(tile).cloned()).collect();
        self.count(found.is_some());
        found
    }

    pub fn store_tiles_flags(&mut self, version: &str, tiles: &[(i32, i32, i32)], rows: &[TileFlags]) {
        if !self.pin(version) || tiles.len() != rows.len() {
            return;
        }
        if self.tiles.len() + rows.len() > TILE_CAP {
            self.tiles.clear();
        }
        for (tile, row) in tiles.iter().zip(rows) {
            self.tiles.insert(*tile, row.clone());
        }
    }

    pub fn in_f2p(&mut self, version: &str, tiles: &[(i32, i32, i32)]) -> Option<Vec<bool>> {
        if !self.pin(version) {
            return None;
        }
        let found: Option<Vec<bool>> = tiles.iter().map(|tile| self.f2p.get(tile).copied()).collect();
        self.count(found.is_some());
        found
    }

    pub fn store_in_f2p(&mut self, version: &str, tiles: &[(i32, i32, i32)], inside: &[bool]) {
        if !self.pin(version) || tiles.len() != inside.len() {
            return;
        }
        if self.f2p.len() + inside.len() > TILE_CAP {
            self.f2p.clear();
        }
        for (tile, verdict) in tiles.iter().zip(inside) {
            self.f2p.insert(*tile, *verdict);
        }
    }

    pub fn toll_fence(&mut self, version: &str, x: i32, z: i32, level: i32, radius: i32) -> Option<Option<(i32, i32, i32)>> {
        if !self.pin(version) {
            return None;
        }
        let found = self.tolls.iter().find_map(|list| list.answer(x, z, level, radius));
        self.count(found.is_some());
        found
    }

    pub fn store_toll_list(&mut self, version: &str, list: TollList) {
        if self.pin(version) {
            push_bounded(&mut self.tolls, list);
        }
    }

    pub fn static_locs_near(&mut self, version: &str, x: i32, z: i32, level: i32, radius: i32) -> Option<Vec<StaticLoc>> {
        if !self.pin(version) {
            return None;
        }
        let found = self.locs.iter().find_map(|list| list.answer(x, z, level, radius));
        self.count(found.is_some());
        found
    }

    pub fn store_locs_list(&mut self, version: &str, list: LocsList) {
        if self.pin(version) {
            push_bounded(&mut self.locs, list);
        }
    }

    /// Whole-batch walk costs for (source, target set, options) under
    /// this version, in `dests` order. Never per target: a batch's
    /// blocked-target search depends on the whole set.
    pub fn walk_costs(&mut self, version: &str, from: Tile, dests: &[Tile], options: &NavOptions) -> Option<Vec<Option<u32>>> {
        if !self.pin(version) {
            return None;
        }
        let set = costs_set(dests);
        let found = self.costs.get(&(PlanKey::of(from, from, options, version), set.clone())).map(|by_set| {
            dests
                .iter()
                .map(|dest| set.iter().position(|tile| tile == dest).and_then(|slot| by_set[slot]))
                .collect()
        });
        self.count(found.is_some());
        found
    }

    pub fn store_walk_costs(&mut self, version: &str, from: Tile, dests: &[Tile], options: &NavOptions, costs: &[Option<u32>]) {
        if !self.pin(version) || dests.len() != costs.len() {
            return;
        }
        if self.costs.len() >= COSTS_CAP {
            self.costs.clear();
        }
        let set = costs_set(dests);
        let by_set = set.iter().map(|tile| dests.iter().position(|dest| dest == tile).and_then(|index| costs[index])).collect();
        self.costs.insert((PlanKey::of(from, from, options, version), set), by_set);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(x: i32, z: i32) -> TileFlags {
        TileFlags { tile: Tile { x, z, level: 0 }, flags: 7, door: None, toll: false, loc: None }
    }

    /// Whole batch or nothing: a partial batch misses (goes out as
    /// before), a complete one hits, in request order.
    #[test]
    fn tiles_hit_only_when_the_whole_batch_is_known() {
        let mut cache = StaticCache::default();
        assert_eq!(cache.tiles_flags("v1", &[(1, 2, 0)]), None, "cold");
        cache.store_tiles_flags("v1", &[(1, 2, 0), (3, 4, 0)], &[flags(1, 2), flags(3, 4)]);
        assert_eq!(cache.tiles_flags("v1", &[(3, 4, 0), (1, 2, 0)]), Some(vec![flags(3, 4), flags(1, 2)]));
        assert_eq!(cache.tiles_flags("v1", &[(3, 4, 0), (5, 6, 0)]), None, "one unknown tile misses");
        assert_eq!(cache.stats, StaticCacheStats { hits: 1, misses: 2 });
    }

    /// A new version drops every answer; no version never caches.
    #[test]
    fn version_change_invalidates_and_empty_version_never_caches() {
        let mut cache = StaticCache::default();
        cache.store_in_f2p("v1", &[(1, 2, 0)], &[true]);
        assert_eq!(cache.in_f2p("v1", &[(1, 2, 0)]), Some(vec![true]));
        assert_eq!(cache.in_f2p("v2", &[(1, 2, 0)]), None, "reload drops old answers");
        assert_eq!(cache.in_f2p("v1", &[(1, 2, 0)]), None, "and they never come back");
        cache.store_in_f2p("", &[(1, 2, 0)], &[true]);
        assert_eq!(cache.in_f2p("", &[(1, 2, 0)]), None, "unpinned: always remote");
    }

    /// Covering lists answer inside their diamond only.
    #[test]
    fn toll_list_answers_inside_its_diamond() {
        let mut cache = StaticCache::default();
        cache.store_toll_list("v1", TollList { x: 3250, z: 3227, level: 0, radius: 150 + TOLL_LIST_MARGIN, gates: vec![(3268, 3227, 0)] });
        assert_eq!(cache.toll_fence("v1", 3260, 3230, 0, 150), Some(Some((3268, 3227, 0))));
        assert_eq!(cache.toll_fence("v1", 3250 + TOLL_LIST_MARGIN + 1, 3227, 0, 150), None, "outside the list");
        assert_eq!(cache.toll_fence("v1", 3250, 3227, 1, 150), None, "other level");
    }

    /// Walk costs memo: re-ordered window hits, other options or source
    /// miss.
    #[test]
    fn walk_costs_memo_keys_on_source_set_and_options() {
        let mut cache = StaticCache::default();
        let from = Tile { x: 1, z: 1, level: 0 };
        let (near, far) = (Tile { x: 2, z: 1, level: 0 }, Tile { x: 9, z: 9, level: 0 });
        let options = NavOptions::default();
        cache.store_walk_costs("v1", from, &[far, near], &options, &[Some(20), Some(1)]);
        assert_eq!(cache.walk_costs("v1", from, &[near, far, near], &options), Some(vec![Some(1), Some(20), Some(1)]));
        let mut paid = NavOptions::default();
        paid.allow_paid = true;
        assert_eq!(cache.walk_costs("v1", from, &[near, far], &paid), None, "options are part of the key");
        assert_eq!(cache.walk_costs("v1", near, &[near, far], &options), None, "source is part of the key");
    }
}

#[cfg(test)]
#[path = "static_cache_live_tests.rs"]
mod live_tests;
