//! Banks ranked by route cost (tour 2, N3).
//!
//! The bank serving a site (or the bot) was the nearest booth as the crow
//! flies. Since every site is a site like any other (D2), the Lumbridge
//! swamp mine took 140 runs an hour, all served by Al Kharid: 27 tiles
//! away on the map, 467 walked around the river (Draynor: 170). Walk
//! timeouts went from 2.3 to 24 an hour, plus 38 approach NoRoute.
//!
//! The candidates are the `BANK_CANDIDATES` nearest banks by Manhattan
//! (one booth per bank), priced in ONE `walk_costs` batch from the
//! source; the cheapest route wins, an unplannable bank never does. A
//! batch with no cost at all (service down, refused) keeps the Manhattan
//! order, the `walk_costs` contract: the walk itself then fails loud
//! with `nav-down`.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::plan::Tile;
use lifescape_sdk::Bot;

/// Banks priced per pick: the swamp mine's route winner (Draynor) is
/// only its second bank by Manhattan; four leave room for a river or a
/// wall between the site and its nearest booths.
pub const BANK_CANDIDATES: usize = 4;
/// Booths closer than this to an already kept booth belong to the same
/// bank (a counter of booths side by side).
const SAME_BANK_TILES: i64 = 12;

/// One booth per bank, the `BANK_CANDIDATES` nearest to (`x`, `z`) by
/// Manhattan among `banks`, nearest first.
pub fn candidates(x: i64, z: i64, banks: &[knowledge::banks::Bank]) -> Vec<knowledge::banks::Bank> {
    let mut sorted: Vec<&knowledge::banks::Bank> = banks.iter().collect();
    sorted.sort_by_key(|bank| ((bank.x - x).abs() + (bank.z - z).abs(), bank.x, bank.z));
    let mut kept: Vec<knowledge::banks::Bank> = Vec::new();
    for bank in sorted {
        if kept.iter().any(|known| (known.x - bank.x).abs().max((known.z - bank.z).abs()) <= SAME_BANK_TILES) {
            continue;
        }
        kept.push(bank.clone());
        if kept.len() == BANK_CANDIDATES {
            break;
        }
    }
    kept
}

/// Pure pick: the cheapest priced candidate; an unpriced one (no route)
/// never wins over a priced one; nothing priced keeps the first
/// (Manhattan order).
pub fn pick_by_cost(candidates: &[knowledge::banks::Bank], costs: &[Option<u32>]) -> Option<knowledge::banks::Bank> {
    let priced = candidates.iter().zip(costs.iter()).filter_map(|(bank, cost)| cost.map(|cost| (cost, bank))).min_by_key(|(cost, _)| *cost);
    match priced {
        Some((_, bank)) => Some(bank.clone()),
        None => candidates.first().cloned(),
    }
}

/// Tile a route to the booth ends on: its first client stand (the booth
/// itself is a blocked counter tile).
fn booth_target(bank: &knowledge::banks::Bank) -> Tile {
    let stand = knowledge::bank_stands::client_stands(bank.x, bank.z, bank.level).and_then(|stands| stands.first().copied());
    let (x, z) = stand.unwrap_or((bank.x, bank.z));
    Tile { x: x as i32, z: z as i32, level: bank.level as i32 }
}

/// The bank of `banks` with the cheapest route from `from`, candidates
/// pre-ranked from (`anchor_x`, `anchor_z`).
pub fn cheapest_route_bank(bot: &mut Bot, from: Tile, anchor: (i64, i64), banks: &[knowledge::banks::Bank]) -> Option<knowledge::banks::Bank> {
    let near = candidates(anchor.0, anchor.1, banks);
    if near.len() <= 1 {
        return near.first().cloned();
    }
    let targets: Vec<Tile> = near.iter().map(booth_target).collect();
    let costs = bot.walk_costs_from(from, &targets);
    pick_by_cost(&near, &costs)
}

/// Free banks a site on `level` banks at: its own level when
/// it has one, else the surface (mill top, castle floors).
pub fn free_banks_for_level(level: i64) -> Vec<knowledge::banks::Bank> {
    let all = knowledge::banks::all_banks();
    let wanted = if all.iter().any(|bank| !bank.members && bank.level == level) { level } else { 0 };
    all.into_iter().filter(|bank| !bank.members && bank.level == wanted).collect()
}

#[cfg(test)]
mod tests {
    use super::{candidates, pick_by_cost};
    use lifescape_gamedata::banks::Bank;

    fn bank(x: i64, z: i64) -> Bank {
        Bank { x, z, level: 0, loc_id: 2213, name: "bankbooth".to_string(), members: false }
    }

    #[test]
    fn one_booth_per_bank_nearest_first() {
        let banks = vec![bank(3269, 3167), bank(3269, 3169), bank(3092, 3243), bank(3253, 3420), bank(3185, 3436), bank(3013, 3355)];
        let near = candidates(3244, 3165, &banks);
        assert_eq!(near.len(), 4);
        assert_eq!((near[0].x, near[0].z), (3269, 3167), "Al Kharid first by Manhattan");
        assert_eq!((near[1].x, near[1].z), (3092, 3243), "its twin booth is skipped");
    }

    /// Red test N3: the swamp mine's route prices Draynor 170, Al Kharid
    /// 467: Draynor serves; an unplannable bank never wins; no price at
    /// all keeps the Manhattan order.
    #[test]
    fn the_cheapest_route_wins_over_the_nearest_booth() {
        let near = vec![bank(3269, 3167), bank(3092, 3243)];
        let picked = pick_by_cost(&near, &[Some(467), Some(170)]).expect("a bank");
        assert_eq!((picked.x, picked.z), (3092, 3243));
        let picked = pick_by_cost(&near, &[None, Some(900)]).expect("a bank");
        assert_eq!((picked.x, picked.z), (3092, 3243), "no route never wins");
        let picked = pick_by_cost(&near, &[None, None]).expect("Manhattan order");
        assert_eq!((picked.x, picked.z), (3269, 3167));
    }

    /// Live N3 against the real map: the swamp mine (3244,3165) is served
    /// by Draynor, never Al Kharid, whether the brain fixed the site or
    /// the bot stands on it.
    #[test]
    fn the_swamp_mine_banks_at_draynor() {
        let (mut bot, _service) = crate::served_nav_bot("bank-route-swamp");
        bot.allow_wild = true;
        bot.world.level = 0;
        bot.world.x = 3244;
        bot.world.z = 3165;
        bot.assigned_site = Some((3244, 3165, 0));
        let served = crate::kit::site::site_bank(&mut bot).expect("a bank serves the mine");
        assert!((served.x - 3092).abs() <= 6 && (served.z - 3243).abs() <= 6, "Draynor serves the site, got {served:?}");
        let nearest = crate::kit::bank::nearest_bank(&mut bot).expect("a bank from the mine");
        assert!((nearest.x - 3092).abs() <= 6 && (nearest.z - 3243).abs() <= 6, "Draynor from the bot, got {nearest:?}");
    }
}
