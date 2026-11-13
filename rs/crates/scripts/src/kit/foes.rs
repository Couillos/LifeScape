//! Foe acquisition for the fight loops (melee, ranged, magic): spawn
//! clusters of a combat spot, the shop-staff guard, the walking-cost
//! order of the acquisition window and the door-aware attack click.
//! Split out of `kit::spots` (rule 4); re-exported there.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bans::{same_side, BanSet};
use crate::kit::spots::{BLOCKED_BAN_TICKS, DOOR_BAN_TICKS};
use crate::ScriptError;
// -- melee target acquisition (foe clusters, ranking, shop-staff guard) --
// Pools of a different kind: NPC groups instead of resource tiles. Shares
// the walk-refinement and flank filters with the gather pools above.

fn nearest_bank_tile(bot: &Bot) -> Option<(i32, i32, i32)> {
    knowledge::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level == i64::from(bot.world.level))
        .min_by_key(|bank| (bank.x - i64::from(bot.world.x)).abs() + (bank.z - i64::from(bot.world.z)).abs())
        .map(|bank| (bank.level as i32, bank.x as i32, bank.z as i32))
}

/// NPC spawn clusters for a spot (same-level spawns only).
pub fn ranked_clusters(bot: &Bot, spot: &knowledge::combat_spots::CombatSpot) -> Vec<(i32, i32, usize)> {
    let mut spawns: Vec<(i64, i64)> = Vec::new();
    for entry in knowledge::all_npcs() {
        let name = entry.name.unwrap_or_default();
        if !spot.npc_names.contains(&name) || entry.members == Some(true) {
            continue;
        }
        for spawn in entry.spawns {
            // shortcut: same-level spawns only; the fighter never rides
            // down to a dungeon camp. Revisit if one is wanted.
            if spawn.level != i64::from(bot.world.level) {
                continue;
            }
            // Members land and the Wilderness are never hunting grounds:
            // the bank-distance rank alone let a far camp (wild hill
            // giants, Crandor hobgoblins) win when nothing nearer stood.
            if !knowledge::free_zones::is_free(spawn.x, spawn.z)
                || knowledge::melee_ladder::in_wilderness_any(spawn.x, spawn.z, spawn.level)
            {
                continue;
            }
            spawns.push((spawn.x, spawn.z));
        }
    }
    let mut groups: Vec<Vec<(i64, i64)>> = Vec::new();
    for spawn in spawns {
        let mut placed = false;
        for group in groups.iter_mut() {
            let center_x: i64 = group.iter().map(|item| item.0).sum::<i64>() / group.len() as i64;
            let center_z: i64 = group.iter().map(|item| item.1).sum::<i64>() / group.len() as i64;
            if (spawn.0 - center_x).abs().max((spawn.1 - center_z).abs()) <= 15 {
                group.push(spawn);
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(vec![spawn]);
        }
    }
    let mut ranked: Vec<(f64, i32, i32, usize)> = groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| {
            let center_x = (group.iter().map(|item| item.0).sum::<i64>() / group.len() as i64) as i32;
            let center_z = (group.iter().map(|item| item.1).sum::<i64>() / group.len() as i64) as i32;
            let dist = nearest_bank_tile(bot).map(|(_, bank_x, bank_z)| {
                (bank_x - center_x).abs() + (bank_z - center_z).abs()
            }).unwrap_or(10000) as f64;
            (group.len() as f64 - dist / 40.0, center_x, center_z, group.len())
        })
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(_, x, z, count)| (x, z, count)).collect()
}

/// Shop and bank staff are never targets: keepers, assistants, Bob and
/// bankers (some at 1 HP) must survive our hunts. Excluded by op
/// category — any Trade/Bank right-click — never by name, so a new shop
/// inherits the guard. Valid foes carry Attack instead (goblin npc 100:
/// [None, Attack, None, ...]).
/// Source (read-only): gamedata npcs.json ops, e.g. generalshopkeeper1
/// (npc 520) and generalassistant1 (521) carry Trade at op index 2.
pub fn is_shop_staff(npc_type: u16) -> bool {
    knowledge::npc_by_id(usize::from(npc_type)).is_some_and(|entry| {
        entry.ops.iter().flatten().any(|op| {
            let lowered = op.to_lowercase();
            lowered.contains("trade") || lowered.contains("bank")
        })
    })
}

/// Acquisition window bound: Manhattan ranks the pool, only the top-K get
/// a pure plan (see `lifescape_nav::walk_rank`), so never more plans than
/// approaches. K=3: foes die fast, the window re-ranks every pick.
const FOE_SHORTLIST: usize = 3;

/// Walking-cost order for the acquisition window (remote batch, no
/// walk, no send): Manhattan lies across rivers and fences, so the
/// window is reordered by service plan cost before the door-aware
/// approach verifies reachability by walking. Unwired or unplannable
/// keeps the Manhattan order; the sort itself lives in
/// `lifescape_nav::walk_rank`.
pub fn rank_foes(bot: &mut Bot, window: Vec<(u16, i32, i32)>) -> Vec<(u16, i32, i32)> {
    let level = bot.world.level;
    crate::skill_walk::refine_walking(bot, window, FOE_SHORTLIST, "foe", |(_, x, z)| {
        lifescape_nav::plan::Tile { x: *x, z: *z, level }
    })
    .into_iter()
    .map(|(_, cand)| cand)
    .collect()
}

pub fn attack_nearest(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    avoid: &mut BanSet,
) -> Result<Option<u16>, ScriptError> {
    let names: Vec<&str> = spot.npc_names.to_vec();
    let mut cands = bot.npcs_named(&names);
    cands.retain(|(_, x, z)| !avoid.is_banned((*x, *z), bot.world.ticks));
    // Another player's foe: skipped before the walk, not after the refusal.
    cands.retain(|(slot, _, _)| !bot.npc_engaged_by_other(*slot));
    // Shop staff are never foes, by op category (Trade/Bank), not name:
    // a keeper next to the hunt (Bob by the axe shop, the Lumbridge
    // general store pair) is never a valid click.
    cands.retain(|(slot, _, _)| {
        bot.world.npcs.get(slot).is_none_or(|npc| !is_shop_staff(npc.npc_type))
    });
    // Never target across the toll fence: goblins spawn on both flanks,
    // so the nearest flips sides every kill and each crossing walks the
    // ~200-tile foot detour (paid doors stay off all hunt). One side per
    // pick; the flank follows the bot, not the plan, so no replan loop.
    if let Some(line) = crate::kit::bank::toll_gate_x(bot) {
        let home = bot.world.x;
        cands.retain(|(_, x, _)| same_side(line, *x, home));
    }
    cands.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
    // Door-aware acquisition, first reachable wins (max 3 foes): the window
    // above is walking-cost ordered (rank_foes), so the first approach is
    // the nearest by foot, not by air. Stand orthogonally adjacent via the
    // router, which opens shut doors on the way (travel Segment::OpenDoor).
    // NoRoute or a blocked ring bans the tile and tries the next goblin
    // instead of insisting on this one.
    for (slot, x, z) in rank_foes(&mut *bot, cands.into_iter().take(FOE_SHORTLIST).collect()) {
        match bot.approach_npc(slot, 100) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => {
                // Uncrossable door (verified open, step refused): the house
                // is out for this fight, not the foe. One approach burned
                // per tile, never a retry campaign.
                if matches!(error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                    bot.log(&format!("goblin {slot} at ({x},{z}) behind uncrossable door, excluding: {error}"));
                    avoid.ban_until((x, z), DOOR_BAN_TICKS);
                    continue;
                }
                bot.log(&format!("goblin {slot} at ({x},{z}) unreachable: {error}"));
                avoid.ban_for((x, z), bot.world.ticks, BLOCKED_BAN_TICKS);
                continue;
            }
        }
        // Taken while we walked up: next foe, no refused click.
        if bot.npc_engaged_by_other(slot) {
            bot.log(&format!("npc {slot} at ({x},{z}) engaged by another player on arrival, next"));
            continue;
        }
        bot.op_npc(2, slot)?;
        return Ok(Some(slot));
    }
    Ok(None)
}


#[cfg(test)]
mod tests {
    use super::{is_shop_staff, rank_foes};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Shop staff are excluded by op category, never by name: the
    /// Lumbridge general store pair (520/521), Bob (519) and bankers
    /// carry Trade/Bank; every valid foe (goblin 100, chicken 41, man 1)
    /// carries Attack instead.
    #[test]
    fn shop_staff_never_targeted_by_category() {
        assert!(is_shop_staff(520), "Lumbridge keeper");
        assert!(is_shop_staff(521), "Lumbridge assistant");
        assert!(is_shop_staff(519), "Bob the axe seller");
        assert!(is_shop_staff(494), "banker");
        assert!(!is_shop_staff(100), "goblin fights");
        assert!(!is_shop_staff(41), "chicken fights");
        assert!(!is_shop_staff(1), "man fights");
    }

    /// Walking-cost order inside the take(3) window: slot 7 at (3252,3237)
    /// is Manhattan-nearest from (3230,3241) but walks ~50 (houses/doors)
    /// while slot 9 at (3203,3246) walks ~36 — the lure loses, and no walk
    /// was spent to learn it (one remote batch, L1-cached on repeat).
    #[test]
    fn rank_foes_prefers_walk_cheap() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-foes".to_string()),
            &lifescape_nav::service::test_socket("foes-walk"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "foes"));
        bot.plan_data_version = Some("test-foes".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        let ranked = rank_foes(&mut bot, window.clone());
        // Deterministic under parallel load: a served batch puts the
        // walk-cheap foe first, a degraded batch keeps Manhattan order.
        assert!(
            ranked == vec![(9u16, 3203, 3246), (7u16, 3252, 3237)] || ranked == window,
            "walk-cheap first or Manhattan fallback, got: {ranked:?}"
        );
        // L1 hit: the same pair costs no new compute.
        let before = service.stats();
        let again = rank_foes(&mut bot, window.clone());
        assert_eq!(again, ranked, "repeat ranks identically");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "second ranking hits L1, never recomputes");
        assert!(after.hits + after.trunc_hits > before.hits + before.trunc_hits, "L1 served the repeat");
    }

    /// No planner wired (or nothing plannable): the window keeps its
    /// Manhattan order, never empties — ranking changes, picking never.
    #[test]
    fn rank_foes_unwired_keeps_manhattan() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        assert_eq!(rank_foes(&mut bot, window.clone()), window, "unwired keeps Manhattan order");
    }

    /// Operator stop during ranking: cancel, no block, Manhattan order.
    #[test]
    fn rank_foes_stop_returns_manhattan_without_blocking() {
        use std::sync::Arc;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-foes-stop".to_string()),
            &lifescape_nav::service::test_socket("foes-stop"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "foes-stop"));
        bot.plan_data_version = Some("test-foes-stop".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.on_pump = Some(Arc::new(|_: &mut Bot| false));
        let start = std::time::Instant::now();
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        assert_eq!(rank_foes(&mut bot, window.clone()), window, "stop keeps Manhattan order");
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "stop cancels, never blocks");
    }

}
