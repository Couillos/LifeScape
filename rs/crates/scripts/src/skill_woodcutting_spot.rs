//! Woodcutter spot picking: tiers, spawns, liveness, toll fence and
//! hunter filters. Pure selection (no walking): the gather loop walks to
//! whatever this returns. Split from `skill_woodcutting` (file-size rule).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::ScriptError;

const NEAR_BANK_TILES: i64 = 150;

/// Same-tier cluster hop, in tiles (Manhattan, bot -> spot). When every
/// spot of the best tier near the current bank is a stump or banned, walk
/// to the next live cluster of the SAME tier instead of degrading: a yew
/// pays 175 xp/log vs 67.5 for willow (dump `exp` is xp*10), and 200 tiles
/// is ~300 walking ticks (measured ~1.5 ticks/tile on flat legs) — about
/// one stump-ban window (GONE_BAN_TICKS = 250), so the home rotation has
/// barely regrown when the hop lands. Past that the walk outlasts the
/// regrow and willow-at-home wins: degrade instead, logged.
pub(crate) const SAME_TIER_HOP_TILES: i64 = 200;

pub(crate) fn pick_tier(tiers: &[knowledge::trees::TreeTier], level: i64) -> Option<knowledge::trees::TreeTier> {
    let mut best: Option<&knowledge::trees::TreeTier> = None;
    for tier in tiers {
        if tier.level <= level && best.as_ref().is_none_or(|known| tier.level > known.level) {
            best = Some(tier);
        }
    }
    best.cloned()
}

/// Tree spawns for the tier's loc names within range of the bank.
fn tier_spawns(tier: &knowledge::trees::TreeTier, bank: &knowledge::banks::Bank) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.trees.iter().any(|tree| *tree == name) {
            for spawn in &entry.spawns {
                if spawn.level == bank.level
                    && (spawn.x - bank.x).abs() + (spawn.z - bank.z).abs() <= NEAR_BANK_TILES
                {
                    found.push((spawn.x as i32, spawn.z as i32));
                }
            }
        }
    }
    found
}

/// Every spawn of the tier's loc names on this level, any bank: the
/// cluster-hop pool when nothing qualifies near the current bank.
fn tier_spawns_wide(tier: &knowledge::trees::TreeTier, level: i64) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.trees.iter().any(|tree| *tree == name) {
            for spawn in &entry.spawns {
                if spawn.level == level {
                    found.push((spawn.x as i32, spawn.z as i32));
                }
            }
        }
    }
    found
}

/// False when the server cleared this tile or confirmed something else
/// here. A tile that has not streamed yet counts as standing (the static
/// template is the truth until the server says otherwise).
fn tree_live(bot: &Bot, spot_x: i32, spot_z: i32, tier_names: &[String]) -> bool {
    if bot.world.loc_removed(spot_x, spot_z, bot.world.level) {
        return false;
    }
    match bot.world.loc_at(spot_x, spot_z, bot.world.level) {
        Some((loc, _, _)) => knowledge::loc_by_id(usize::from(loc))
            .is_none_or(|entry| entry.name.is_some_and(|name| tier_names.iter().any(|tree| *tree == name.to_lowercase()))),
        None => true,
    }
}

/// X of the nearest toll fence seen so far. Same nearest-gate rule as
/// `nearest_bank`: the fence is a vertical line, spots flip sides with it.
fn toll_line(bot: &Bot) -> Option<i32> {
    let (here_x, here_z) = (bot.world.x, bot.world.z);
    bot.toll_gates
        .iter()
        .filter(|(_, _, level)| *level == bot.world.level)
        .min_by_key(|(gate_x, gate_z, _)| (gate_x - here_x).abs() + (gate_z - here_z).abs())
        .map(|(gate_x, _, _)| *gate_x)
}

/// True when the spot sits across the toll fence from the bot.
fn across_toll(bot_x: i32, spot_x: i32, line: Option<i32>) -> bool {
    line.is_some_and(|fence| (spot_x >= fence) != (bot_x >= fence))
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
fn hunter_zones(bot: &Bot) -> Vec<(i64, i64, i64)> {
    const WANDER: i64 = 2;
    let ours = bot.combat_level();
    let mut zones = Vec::new();
    for entry in knowledge::all_npcs() {
        if !entry.hunts_players || !hunter_threatens(ours, entry.vislevel) {
            continue;
        }
        for spawn in &entry.spawns {
            if spawn.level == i64::from(bot.world.level) {
                zones.push((spawn.x, spawn.z, entry.huntrange + WANDER));
            }
        }
    }
    zones
}

pub(crate) fn pick_tree_spot(
    bot: &Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &HashSet<(i32, i32)>,
) -> Option<(knowledge::trees::TreeTier, i32, i32)> {
    let fence = if bot.allow_paid { None } else { toll_line(bot) };
    // Hunter filter unless opted out: spots a threat could reach are
    // skipped (the hurt-driven flee below still handles wanderers).
    let hunters: Vec<(i64, i64, i64)> = if bot.allow_wild { Vec::new() } else { hunter_zones(bot) };
    let mut saw_paid_only = false;
    let mut affordable: Vec<&knowledge::trees::TreeTier> = tiers.iter().filter(|tier| tier.level <= level).collect();
    affordable.sort_by_key(|tier| -tier.level);
    let best = affordable.first().copied();
    let top_level = best.map(|tier| tier.level);
    // First degraded-tier candidate, held back: the same-tier hop below
    // preempts it, and only a failed hop falls through to it, logged.
    let mut fallback: Option<(knowledge::trees::TreeTier, i32, i32)> = None;
    for tier in affordable {
        let mut nearest: Option<(i64, (i32, i32))> = None;
        for (spot_x, spot_z) in tier_spawns(tier, bank) {
            if banned.contains(&(spot_x, spot_z)) || !tree_live(bot, spot_x, spot_z, &tier.trees) {
                continue;
            }
            if across_toll(bot.world.x, spot_x, fence) {
                saw_paid_only = true;
                continue;
            }
            if hunters.iter().any(|(hx, hz, range)| {
                (i64::from(spot_x) - hx).abs() + (i64::from(spot_z) - hz).abs() <= *range
            }) {
                continue;
            }
            let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
            if nearest.as_ref().is_none_or(|(known, _)| dist < *known) {
                nearest = Some((dist, (spot_x, spot_z)));
            }
        }
        if let Some((_, (spot_x, spot_z))) = nearest {
            if Some(tier.level) == top_level {
                return Some((tier.clone(), spot_x, spot_z));
            }
            if fallback.is_none() {
                fallback = Some((tier.clone(), spot_x, spot_z));
            }
        }
    }
    // Best tier fully stumped/banned near this bank: hop to the next live
    // cluster of the SAME tier before degrading. An unreachable hop bans
    // long (UNREACHABLE_BAN_TICKS in the caller) so the next repick tries
    // the following cluster, then the tier below — no travel loop.
    if let Some(top) = best {
        let mut hop: Option<(i64, (i32, i32))> = None;
        for (spot_x, spot_z) in tier_spawns_wide(top, i64::from(bot.world.level)) {
            if banned.contains(&(spot_x, spot_z)) || !tree_live(bot, spot_x, spot_z, &top.trees) {
                continue;
            }
            if across_toll(bot.world.x, spot_x, fence) {
                saw_paid_only = true;
                continue;
            }
            if hunters.iter().any(|(hx, hz, range)| {
                (i64::from(spot_x) - hx).abs() + (i64::from(spot_z) - hz).abs() <= *range
            }) {
                continue;
            }
            let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
            if dist <= SAME_TIER_HOP_TILES && hop.as_ref().is_none_or(|(known, _)| dist < *known) {
                hop = Some((dist, (spot_x, spot_z)));
            }
        }
        if let Some((dist, (spot_x, spot_z))) = hop {
            bot.log(&format!(
                "no {} near bank ({},{}), hopping same tier to ({spot_x},{spot_z}) +{dist} tiles",
                top.product, bank.x, bank.z
            ));
            return Some((top.clone(), spot_x, spot_z));
        }
        if fallback.is_some()
            && tiers.iter().any(|tier| tier.level < top.level && tier.level <= level)
        {
            bot.log(&format!(
                "no {} reachable under {SAME_TIER_HOP_TILES} tiles, degrading a tier",
                top.product
            ));
        }
    }
    if saw_paid_only {
        bot.log("tree spots only across toll fence");
    }
    fallback
}

/// Live loc id at a tile (live stream first, static template as fallback).
pub(crate) fn live_loc_id(bot: &Bot, tree_x: i32, tree_z: i32) -> Result<u16, ScriptError> {
    if let Some((loc, _, _)) = bot.world.loc_at(tree_x, tree_z, bot.world.level) {
        return Ok(loc);
    }
    Err(ScriptError::Usage(format!("no loc at {tree_x},{tree_z} from {:?}", bot.position())))
}

/// True only when the server confirms a stump here. Unknown means keep
/// waiting, never assume depletion.
pub(crate) fn is_stump(bot: &Bot, tree_x: i32, tree_z: i32) -> bool {
    match bot.world.locs.get(&(tree_x, tree_z, bot.world.level)) {
        Some(Some((loc, _, _))) => knowledge::loc_by_id(usize::from(*loc))
            .is_some_and(|entry| entry.name.unwrap_or_default().contains("stump")),
        _ => false,
    }
}

pub(crate) fn tree_still_there(bot: &Bot, spot_x: i32, spot_z: i32, tier_names: &[String]) -> bool {
    if bot.world.loc_removed(spot_x, spot_z, bot.world.level) {
        return false;
    }
    let Ok(loc) = live_loc_id(bot, spot_x, spot_z) else {
        return true;
    };
    match knowledge::loc_by_id(usize::from(loc)) {
        Some(entry) => entry.name.is_some_and(|name| tier_names.iter().any(|tree| *tree == name.to_lowercase())),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;
    use std::collections::HashSet;

    /// Threat rule mirrors the server: hunters skip players above twice
    /// their level, so combat 4 still fears spiders (2) but combat 5
    /// walks past them; unknown own level counts as threatened.
    #[test]
    fn hunter_threat_follows_server_rule() {
        assert!(hunter_threatens(Some(4), 2), "spider hunts combat 4");
        assert!(!hunter_threatens(Some(5), 2), "combat 5 too strong for spiders");
        assert!(hunter_threatens(Some(4), 26), "jailguard hunts everyone here");
        assert!(hunter_threatens(None, 1), "unknown level stays cautious");
    }

    /// Toll fence: with `allow_paid == false` every spot across the line is
    /// skipped (log + None, no silent fallback); with `true` the same query
    /// returns a spot. Draynor bank, bot far west, fence between.
    #[test]
    fn toll_fence_filters_across_spots() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 2900;
        bot.world.z = 3240;
        bot.toll_gates.insert((2920, 3240, 0));
        assert_eq!(toll_line(&bot), Some(2920), "nearest fence line");
        assert!(across_toll(2900, 3086, Some(2920)), "east spot is across");
        assert!(!across_toll(2900, 2910, Some(2920)), "west spot is same side");
        assert!(!across_toll(2900, 3086, None), "no fence means no filtering");
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        bot.allow_paid = false;
        let hopped = pick_tree_spot(&bot, &tiers, &bank, 0, &HashSet::new());
        let Some((_, hop_x, _)) = hopped else {
            panic!("hop should find a same-side tree, got None");
        };
        assert!(hop_x < 2920, "hop stays on the bot side of the fence, got x={hop_x}");
        bot.allow_paid = true;
        let some = pick_tree_spot(&bot, &tiers, &bank, 0, &HashSet::new());
        assert!(some.is_some(), "paid crossing reopens the same spots");
    }

    /// Same-tier hop: every yew near the Draynor bank is a stump/ban, so
    /// the pick walks to the next live yew cluster ((2996,3311), 97 tiles
    /// off) instead of settling for the live willow next door.
    #[test]
    fn hop_same_tier_before_degrading() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3053;
        bot.world.z = 3271;
        bot.allow_wild = true;
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        let banned: HashSet<(i32, i32)> = [
            (3053, 3271),
            (3041, 3319),
            (3017, 3308),
            (3184, 3226),
            (3146, 3254),
            (3151, 3230),
            (3165, 3219),
        ]
        .into_iter()
        .collect();
        let picked = pick_tree_spot(&bot, &tiers, &bank, 63, &banned);
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("hop should find the next yew cluster, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier, not willow");
        assert_eq!((spot_x, spot_z), (2996, 3311), "nearest live yew cluster, got: ({spot_x},{spot_z})");
    }

    /// Hop bound: with every yew inside SAME_TIER_HOP_TILES banned too,
    /// the pick degrades to willow (logged) instead of marching across
    /// the map for one log.
    #[test]
    fn hop_bound_refuses_far_march() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3053;
        bot.world.z = 3271;
        bot.allow_wild = true;
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        let banned: HashSet<(i32, i32)> = [
            (3053, 3271),
            (3041, 3319),
            (3017, 3308),
            (3184, 3226),
            (3146, 3254),
            (3151, 3230),
            (3165, 3219),
            (2996, 3311),
            (2940, 3232),
            (2933, 3233),
            (2934, 3225),
            (2935, 3229),
        ]
        .into_iter()
        .collect();
        let picked = pick_tree_spot(&bot, &tiers, &bank, 63, &banned);
        let Some((tier, _, _)) = picked else {
            panic!("degrade should find a willow, got None");
        };
        assert_eq!(tier.product, "willow_logs", "bound exceeded, degrade, got: {}", tier.product);
    }
}
