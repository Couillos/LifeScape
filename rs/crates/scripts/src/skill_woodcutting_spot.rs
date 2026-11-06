//! Woodcutter spot picking: tiers, spawns, liveness, toll fence and
//! hunter filters. Pure selection (no walking): the gather loop walks to
//! whatever this returns. Split from `skill_woodcutting` (file-size rule).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::ScriptError;

const NEAR_BANK_TILES: i64 = 150;

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
    for tier in affordable {
        let mut best: Option<(i64, (i32, i32))> = None;
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
            if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                best = Some((dist, (spot_x, spot_z)));
            }
        }
        if let Some((_, (spot_x, spot_z))) = best {
            return Some((tier.clone(), spot_x, spot_z));
        }
    }
    if saw_paid_only {
        bot.log("tree spots only across toll fence");
    }
    None
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
        let none = pick_tree_spot(&bot, &tiers, &bank, 0, &HashSet::new());
        assert!(none.is_none(), "all Draynor spots sit across the fence, got: {none:?}");
        bot.allow_paid = true;
        let some = pick_tree_spot(&bot, &tiers, &bank, 0, &HashSet::new());
        assert!(some.is_some(), "paid crossing reopens the same spots");
    }
}
