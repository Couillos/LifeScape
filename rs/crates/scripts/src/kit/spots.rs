//! Generic spot pool: the filters every gather skill shares, whatever the
//! resource. Pure selection (no walking): the gather loop walks to
//! whatever the caller picks from the survivors.
//!
//! Occupation perçue (new): a spot where the bot *sees* another player
//! working is skipped while free spots exist — like a player would, and
//! indispensable with hundreds of bots. Only own vision ever decides
//! (D22): `bot.world.players` is what this bot sees, nothing shared.
//! When every candidate is occupied the pool is kept whole (never idle
//! while a resource stands).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

/// Chebyshev radius around a spot that reads as worked when another
/// player stands inside it (stands ring the spot at distance 1-2; 3
/// covers a neighbour tree's cutter without shadowing a whole forest).
pub const OCCUPIED_RADIUS: i32 = 3;

/// Visible players on this level: only what this bot sees, never shared.
pub fn visible_workers(bot: &Bot) -> Vec<(i32, i32)> {
    bot.world
        .players
        .values()
        .filter(|player| player.level == bot.world.level)
        .map(|player| (player.x, player.z))
        .collect()
}

/// Drop candidates another visible player stands on (Chebyshev `radius`
/// or under), unless that empties the pool — then keep it whole. Logs
/// the dodge so a crowded hop reads as a decision with a reason.
pub fn drop_occupied(
    bot: &mut Bot,
    cands: Vec<(i64, (i32, i32))>,
    radius: i32,
) -> Vec<(i64, (i32, i32))> {
    if cands.len() <= 1 {
        return cands;
    }
    let workers = visible_workers(bot);
    if workers.is_empty() {
        return cands;
    }
    let free: Vec<(i64, (i32, i32))> = cands
        .iter()
        .filter(|(_, (spot_x, spot_z))| {
            !workers.iter().any(|(worker_x, worker_z)| {
                (worker_x - spot_x).abs().max((worker_z - spot_z).abs()) <= radius
            })
        })
        .copied()
        .collect();
    if free.is_empty() {
        return cands;
    }
    if free.len() != cands.len() {
        bot.log(&format!("{} of {} spots look worked, skipping them", cands.len() - free.len(), cands.len()));
    }
    free
}

/// Shortlist bound for the walking-cost refinement: Manhattan ranks the
/// filtered pool, only the top-K get a pure plan, the cheapest verified
/// walk wins. K=4: live plans cost ~30-500 ms, so <=4 plans per pick
/// (amortized over minutes of gathering); a true-nearest pushed past rank
/// 4 needs a detour the toll filter already excludes.
pub(crate) const WALK_SHORTLIST: usize = 4;

/// Cheapest-to-walk spot among pre-filtered candidates (toll, hunter, F2P,
/// bans and liveness ran BEFORE, so no plan is wasted on an excluded
/// spot). One remote batch (no local Dijkstra, no walk, no send). The
/// ranking lives in `lifescape_nav::walk_rank`; this only bounds the
/// pool, maps the winner back to a walk-or-Manhattan metric and logs the
/// upset. Unwired (no service) or all-disconnected keeps the Manhattan
/// order, never drops the pick.
pub(crate) fn refine_by_walk(bot: &mut Bot, mut cands: Vec<(i64, (i32, i32))>) -> Option<(i64, (i32, i32))> {
    cands.sort();
    let manhattan = *cands.first()?;
    let level = bot.world.level;
    let (cost, (_, pos)) = crate::skill_walk::refine_walking(bot, cands, WALK_SHORTLIST, "spot", |(_, pos)| lifescape_nav::plan::Tile { x: pos.0, z: pos.1, level }).into_iter().next().expect("non-empty pool ranks");
    Some(match cost {
        Some(walk) => (i64::from(walk), pos),
        None => manhattan,
    })
}

/// True when a hunter of this level comes for us: the server skips
/// players above twice its level, so anything at or under that line is
/// a threat (unknown own level counts as threatened).
fn hunter_threatens(our_combat: Option<u8>, vislevel: i64) -> bool {
    our_combat.is_none_or(|ours| i64::from(ours) <= vislevel * 2)
}

/// Static hunter spawns on this level that would attack us: (x, z, aggro
/// range + wander margin). NPCs wander off their spawn, so the raw
/// huntrange alone under-covers; the hurt-driven flee still handles what
/// wanders in.
pub(crate) fn hunter_zones(bot: &Bot) -> Vec<(i64, i64, i64)> {
    const WANDER: i64 = 2;
    let ours = bot.combat_level();
    let mut zones = Vec::new();
    for entry in knowledge::all_npcs() {
        if !entry.hunts_players || !hunter_threatens(ours, entry.vislevel) {
            continue;
        }
        for spawn in entry.spawns {
            if spawn.level == i64::from(bot.world.level) {
                zones.push((spawn.x, spawn.z, entry.huntrange + WANDER));
            }
        }
    }
    zones
}

/// True when a hunter zone covers the tile (Manhattan, like the pickers).
pub(crate) fn hunter_covers(hunters: &[(i64, i64, i64)], spot_x: i32, spot_z: i32) -> bool {
    hunters.iter().any(|(hunter_x, hunter_z, range)| {
        (i64::from(spot_x) - hunter_x).abs() + (i64::from(spot_z) - hunter_z).abs() <= *range
    })
}

/// One F2P verdict per candidate spot, in order, in a single service
/// batch. `None` when the service is down (logged): callers drop every
/// candidate instead of guessing — the pick returns `None` and the loop
/// waits, while any real walk still fails loud with `nav-down`.
pub(crate) fn f2p_inside(bot: &mut Bot, spots: &[(i32, i32)]) -> Option<Vec<bool>> {
    if spots.is_empty() {
        return Some(Vec::new());
    }
    let level = bot.world.level;
    let tiles: Vec<(i32, i32, i32)> = spots.iter().map(|(spot_x, spot_z)| (*spot_x, *spot_z, level)).collect();
    match bot.in_f2p_batch(&tiles) {
        Ok(verdicts) => Some(verdicts),
        Err(error) => {
            bot.log(&format!("f2p batch unavailable ({error}), dropping {} spots", spots.len()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{drop_occupied, hunter_threatens, visible_workers, OCCUPIED_RADIUS};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Threat rule (server skips players above twice the hunter level):
    /// at or under the line is a threat, unknown level stays cautious.
    #[test]
    fn hunter_threat_follows_server_rule() {
        assert!(hunter_threatens(Some(4), 2), "spider hunts combat 4");
        assert!(!hunter_threatens(Some(5), 2), "combat 5 too strong for spiders");
        assert!(hunter_threatens(Some(4), 26), "jailguard hunts everyone here");
        assert!(hunter_threatens(None, 1), "unknown level stays cautious");
    }

    /// Occupation is own vision only: a stranger on the spot dodges it,
    /// nobody around changes nothing, and a full house keeps the pool
    /// (never idle while a resource stands).
    #[test]
    fn occupation_dodges_visible_workers_only() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let cands = vec![(10i64, (100, 100)), (20, (200, 200)), (30, (300, 300))];
        assert_eq!(drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS).len(), 3, "nobody around changes nothing");
        assert!(visible_workers(&bot).is_empty(), "nobody seen");
        bot.world.players.insert(7u16, lifescape_sdk::world::PlayerInfo { x: 101, z: 100, level: 0, jump: false, name: None });
        let free = drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS);
        assert_eq!(free, vec![(20i64, (200, 200)), (30, (300, 300))], "worked spot dodged, got: {free:?}");
        bot.world.players.insert(8u16, lifescape_sdk::world::PlayerInfo { x: 200, z: 200, level: 1, jump: false, name: None });
        let still = drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS);
        assert_eq!(still.len(), 2, "other-level players don't count, got: {still:?}");
        bot.world.players.insert(9u16, lifescape_sdk::world::PlayerInfo { x: 201, z: 199, level: 0, jump: false, name: None });
        bot.world.players.insert(10u16, lifescape_sdk::world::PlayerInfo { x: 300, z: 301, level: 0, jump: false, name: None });
        assert_eq!(drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS), cands, "full house keeps the pool");
        assert_eq!(drop_occupied(&mut bot, vec![(5i64, (1, 1))], OCCUPIED_RADIUS).len(), 1, "single candidate never dropped");
    }
}
