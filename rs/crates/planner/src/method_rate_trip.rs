//! Geography of the rates prior (`method_rate`, split for rule 4): the
//! bank walk of a site, the resources sharing a site, the essence trip.
//! - bank walk: Chebyshev at one tile a tick to the nearest F2P bank; an
//!   underground site (z ≥ 6400) climbs through the nearest link of
//!   `gamedata::underground` (ladder, stairs, trapdoor), never across
//!   the 6400-tile gap in a straight line;
//! - site cluster: the way's resources within `CLUSTER_TILES` of the
//!   worked one (a depleted rock or felled tree sends the player to the
//!   next one; all of them empty, he waits a respawn);
//! - essence: the mine is past Aubury's teleport (`essence_mine.rs2`):
//!   bank to Aubury and back, his teleport `p_delay(4)` (6 ticks), the
//!   exit portal `p_delay(1)` (3 ticks), one client op each, and the walk
//!   from the mine's centre (the arrival square) to a rock and back.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_gamedata::underground::{is_underground, links};

use crate::method_rate::WALK_TILES_PER_TICK;

/// Resources of a way within this Chebyshev reach of the worked one
/// share its load.
pub const CLUSTER_TILES: i64 = 10;
/// Aubury's teleport: `p_delay(4)` then the jump (n + 2 ticks).
pub const AUBURY_TELEPORT_TICKS: f64 = 6.0;
/// The essence mine's exit portal: `p_delay(1)` then the jump.
pub const ESSENCE_PORTAL_TICKS: f64 = 3.0;

fn chebyshev(from: (i64, i64, i64), to: (i64, i64, i64)) -> i64 {
    (from.0 - to.0).abs().max((from.1 - to.1).abs())
}

fn surface_bank_walk(site: (i64, i64, i64)) -> Option<i64> {
    let bank = lifescape_gamedata::banks::nearest_free_bank(site.0, site.1, site.2)?;
    Some((bank.x - site.0).abs().max((bank.z - site.1).abs()))
}

/// One-way walk in tiles from `site` to the nearest F2P bank, through
/// the nearest surface link when `site` is underground.
pub fn bank_walk_tiles(site: (i64, i64, i64)) -> Option<i64> {
    if !is_underground(site.1) {
        return surface_bank_walk(site);
    }
    links().iter().filter_map(|link| surface_bank_walk(link.above).map(|above| chebyshev(site, link.below) + above)).min()
}

/// Round-trip walk from `site` to the nearest F2P bank, in ticks.
pub fn bank_round_trip(site: (i64, i64, i64)) -> Option<f64> {
    // Static banks and links: once per site (every prior of a banking
    // way asks it).
    static MEMO: OnceLock<Mutex<HashMap<(i64, i64, i64), Option<f64>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&site) {
        return *known;
    }
    let found = bank_walk_tiles(site).map(|tiles| 2.0 * tiles as f64 / WALK_TILES_PER_TICK);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(site, found);
    found
}

/// The resources sharing a worked site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cluster {
    /// Resources within `CLUSTER_TILES` of the site (the site's own).
    pub count: f64,
    /// Tiles from the worked resource to its nearest neighbour (0 alone).
    pub step: f64,
}

/// Cluster of `site` among `sites` (memoised: static game data, a tree
/// way lists thousands of tiles).
pub fn cluster(key: &str, sites: &[(i64, i64, i64)], site: (i64, i64, i64)) -> Cluster {
    static MEMO: OnceLock<Mutex<HashMap<(String, (i64, i64, i64)), Cluster>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let memo_key = (key.to_string(), site);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&memo_key) {
        return *known;
    }
    let near: Vec<(i64, i64, i64)> = sites.iter().copied().filter(|other| other.2 == site.2 && chebyshev(*other, site) <= CLUSTER_TILES).collect();
    // The worked resource: the cluster's nearest to the site.
    let worked = near.iter().copied().min_by_key(|other| chebyshev(*other, site)).unwrap_or(site);
    let step = near.iter().filter(|other| **other != worked).map(|other| chebyshev(*other, worked)).min().unwrap_or(0) as f64;
    let found = Cluster { count: near.len().max(1) as f64, step };
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(memo_key, found);
    found
}

/// Extra ticks each success costs on a depleting resource: `fell` odds
/// it empties the resource, the walk to the next one (`step`, the first
/// roll on it one tick earlier than a continued roll, the click one tick
/// later), `settle` ticks the script delays after the success, and the
/// respawn bound (the cluster yields at most `count / (fell × respawn)`
/// per tick). `base` = ticks per success without depletion.
pub fn depletion_ticks(base: f64, fell: f64, respawn: f64, settle: f64, cluster: Cluster) -> f64 {
    let walked = base + fell * (cluster.step + settle);
    let bound = fell * respawn / cluster.count;
    walked.max(bound) - base
}

/// The site a load works best from when none is asked: the one whose
/// bank walk (shared by a pack) plus the cost of each success on its
/// cluster (`depletion_ticks` over `base` ticks a success) is the
/// least; a lone rock beside a bank loses to a field of them further.
/// Memoised per `key` (static game data).
pub fn best_work_site(key: &str, sites: &[(i64, i64, i64)], base: f64, fell: f64, respawn: f64, settle: f64) -> Option<(i64, i64, i64)> {
    static MEMO: OnceLock<Mutex<HashMap<String, Option<(i64, i64, i64)>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let memo_key = format!("{key}|{base}|{fell}|{respawn}|{settle}");
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&memo_key) {
        return *known;
    }
    let pack = crate::method_rate::PACK_SLOTS - 1.0;
    let found = work_sites(key, sites)
        .iter()
        .map(|(site, trip, near)| (*site, trip / pack + base + depletion_ticks(base, fell, respawn, settle, *near)))
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(site, _)| site);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(memo_key, found);
    found
}

/// Sites of `key` a bank serves, in order, with their bank round trip
/// and their cluster (static game data, built once per key: the
/// level-dependent search above only does arithmetic on it; `cluster`
/// already holds one site list per key).
fn work_sites(key: &str, sites: &[(i64, i64, i64)]) -> std::sync::Arc<Vec<((i64, i64, i64), f64, Cluster)>> {
    static MEMO: OnceLock<Mutex<HashMap<String, std::sync::Arc<Vec<((i64, i64, i64), f64, Cluster)>>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(key) {
        return known.clone();
    }
    let table: Vec<((i64, i64, i64), f64, Cluster)> = sites.iter().filter_map(|site| bank_round_trip(*site).map(|trip| (*site, trip, cluster(key, sites, *site)))).collect();
    let table = std::sync::Arc::new(table);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key.to_string(), table.clone());
    table
}

/// Miners a field of `rocks` rocks yielding `product` feeds (lot 3, live
/// 2026-11-19: four coal rocks read as four seats, "contended" 38 times
/// in two hours). A mined rock yields again after `rock_respawn` ticks
/// (`mine.dbrow` `rock_respawnrate`: coal 100); a miner at the rock's
/// level with the best pickaxe that level wields (`guide_methods::
/// tool_at`, `pickaxe_checker.rs2`: rune from 41) takes a roll every
/// `rate + 1` ticks at `stat_odds` (`mining.rs2`), plus the success
/// delay of a normal rock, per ore. The field seats `rocks × ore ticks / respawn` of them,
/// rounded, at least one, never more than its rocks. `None`: a rock
/// that never depletes (essence) or a product without odds.
pub fn miners_fed(product: &str, rocks: i64) -> Option<i64> {
    use lifescape_gamedata::action_odds::{pickaxe_rate, rock_chance, rock_respawn, stat_odds};
    let respawn = rock_respawn(product)? as f64;
    let (low, high) = rock_chance(product)?;
    let method = format!("gather:mining/{product}");
    let gate = crate::guide_methods::gate_level(&method);
    let level = i64::from(gate);
    let odds = stat_odds(level, low, high);
    if odds <= 0.0 || respawn <= 0.0 {
        return None;
    }
    let roll = (pickaxe_rate(&crate::guide_methods::tool_at(&method, "", gate))? + 1) as f64;
    let settle = if crate::method_rate::FAST_ROCKS.contains(&product) { 0.0 } else { crate::method_rate::ROCK_SUCCESS_DELAY_TICKS };
    let rocks = rocks.max(1);
    let fed = (rocks as f64 * (roll / odds + settle) / respawn).round() as i64;
    Some(fed.clamp(1, rocks))
}

/// Round trip of an essence load: bank to Aubury and back, teleport in,
/// walk the mine from its centre to a rock and back, portal out.
pub fn essence_trip(rocks: &[(i64, i64, i64)]) -> Option<f64> {
    let aubury = lifescape_gamedata::services::all_services().into_iter().find(|service| service.key == "aubury_teleport")?;
    let shop = (aubury.x, aubury.z, aubury.level);
    let to_aubury = surface_bank_walk(shop)? as f64;
    if rocks.is_empty() {
        return None;
    }
    let count = rocks.len() as i64;
    let centre = (rocks.iter().map(|rock| rock.0).sum::<i64>() / count, rocks.iter().map(|rock| rock.1).sum::<i64>() / count, rocks[0].2);
    let in_mine = rocks.iter().map(|rock| chebyshev(centre, *rock)).min()? as f64;
    let ops = 2.0 * crate::method_rate::INSTANT_USE_TICKS;
    Some(2.0 * (to_aubury + in_mine) / WALK_TILES_PER_TICK + AUBURY_TELEPORT_TICKS + ESSENCE_PORTAL_TICKS + ops)
}

#[cfg(test)]
mod tests {
    use super::{bank_round_trip, cluster, depletion_ticks, essence_trip, Cluster};

    /// An underground site climbs its ladder: the Mining Guild's coal
    /// banks in Falador through the guild ladder (3020, 9739), not 6400
    /// tiles south in a straight line.
    #[test]
    fn underground_banks_through_its_ladder() {
        let guild = bank_round_trip((3044, 9753, 0)).expect("guild");
        assert!(guild > 40.0 && guild < 200.0, "{guild}");
    }

    /// The essence trip is Aubury's teleport, not a walk from the mine.
    #[test]
    fn essence_trip_rides_the_teleport() {
        let rocks = [(2891, 4847, 0), (2893, 4812, 0), (2925, 4848, 0), (2927, 4814, 0)];
        let trip = essence_trip(&rocks).expect("aubury");
        assert!(trip > super::AUBURY_TELEPORT_TICKS + super::ESSENCE_PORTAL_TICKS && trip < 200.0, "{trip}");
    }

    /// One rock alone: each ore waits the respawn; many rocks: the walk.
    #[test]
    fn depletion_waits_the_respawn_of_a_small_cluster() {
        let lone = depletion_ticks(10.0, 1.0, 200.0, 1.0, Cluster { count: 1.0, step: 0.0 });
        assert!((lone - 190.0).abs() < 1e-9);
        let field = depletion_ticks(10.0, 1.0, 10.0, 1.0, Cluster { count: 6.0, step: 2.0 });
        assert!((field - 3.0).abs() < 1e-9);
        let sites = [(0, 0, 0), (2, 0, 0), (40, 0, 0)];
        assert_eq!(cluster("test", &sites, (0, 0, 0)), Cluster { count: 2.0, step: 2.0 });
    }
}
