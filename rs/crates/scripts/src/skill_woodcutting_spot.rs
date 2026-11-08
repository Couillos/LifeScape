//! Woodcutter spot picking: tiers, spawns, liveness, toll fence and
//! hunter filters. Pure selection (no walking): the gather loop walks to
//! whatever this returns. Split from `skill_woodcutting` (file-size rule).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{GONE_BAN_TICKS, STRUCTURAL_BAN_TICKS};
use crate::ScriptError;

const NEAR_BANK_TILES: i64 = 150;

/// Same-tier cluster hop, in tiles (Manhattan, bot -> spot). When every
/// spot of the best tier near the current bank is a stump or banned, walk
/// to the next live cluster of the SAME tier instead of degrading: a yew
/// pays 175 xp/log vs 67.5 for willow (dump `exp` is xp*10), and 200 tiles
/// is ~200 walking ticks (`travel_ticks`, measured 1.0 ticks/tile) — about
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
    banned: &mut HashMap<(i32, i32), u64>,
) -> Option<(knowledge::trees::TreeTier, i32, i32)> {
    let fence = if bot.allow_paid { None } else { toll_line(bot) };
    // Hunter filter unless opted out: spots a threat could reach are
    // skipped (the hurt-driven flee below still handles wanderers).
    let hunters: Vec<(i64, i64, i64)> = if bot.allow_wild { Vec::new() } else { hunter_zones(bot) };
    let mut saw_paid_only = false;
    let mut saw_f2p_only = false;
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
            if banned.contains_key(&(spot_x, spot_z)) || !tree_live(bot, spot_x, spot_z, &tier.trees) {
                continue;
            }
            if across_toll(bot.world.x, spot_x, fence) {
                saw_paid_only = true;
                continue;
            }
            // Travel always plans F2P-only: an out-of-F2P spot is a
            // guaranteed NoRoute dossier (live t01-4572), never a pick.
            if bot.nav_world.as_ref().is_some_and(|world| !world.in_f2p(spot_x, spot_z)) {
                saw_f2p_only = true;
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
    // cluster of the SAME tier before degrading. A hop that fails to walk
    // rides the short ban (Gone, 250t) so the next repick tries the
    // following cluster, then the tier below — no travel loop.
    if let Some(top) = best {
        let mut hop: Option<(i64, (i32, i32))> = None;
        for (spot_x, spot_z) in tier_spawns_wide(top, i64::from(bot.world.level)) {
            if banned.contains_key(&(spot_x, spot_z)) || !tree_live(bot, spot_x, spot_z, &top.trees) {
                continue;
            }
            if across_toll(bot.world.x, spot_x, fence) {
                saw_paid_only = true;
                continue;
            }
            if bot.nav_world.as_ref().is_some_and(|world| !world.in_f2p(spot_x, spot_z)) {
                saw_f2p_only = true;
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
        if hop.is_none() {
            // One debug line per failed best-tier repick: every in-range
            // spot with its disqualification reason (banned-remaining,
            // stump, gone, toll, hunter). The in-range pool is small (a
            // dozen spots); far ones collapse to a count. A "hop-missed"
            // tag would flag a filter/hop disagreement bug.
            let now = bot.world.ticks;
            let mut wide = tier_spawns_wide(top, i64::from(bot.world.level));
            wide.sort();
            wide.dedup();
            let mut notes: Vec<String> = Vec::new();
            let mut far = 0u32;
            let mut banned_notes: Vec<((i32, i32), u64)> = Vec::new();
            for &(spot_x, spot_z) in &wide {
                let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
                if dist > SAME_TIER_HOP_TILES {
                    far += 1;
                    continue;
                }
                let why = if let Some(until) = banned.get(&(spot_x, spot_z)) {
                    banned_notes.push(((spot_x, spot_z), *until));
                    format!("banned-{}t", until.saturating_sub(now))
                } else if !tree_live(bot, spot_x, spot_z, &top.trees) {
                    if is_stump(bot, spot_x, spot_z) {
                        "stump".to_string()
                    } else {
                        "gone".to_string()
                    }
                } else if across_toll(bot.world.x, spot_x, fence) {
                    "toll".to_string()
                } else if bot.nav_world.as_ref().is_some_and(|world| !world.in_f2p(spot_x, spot_z)) {
                    "f2p".to_string()
                } else if hunters.iter().any(|(hunter_x, hunter_z, range)| {
                    (i64::from(spot_x) - hunter_x).abs() + (i64::from(spot_z) - hunter_z).abs() <= *range
                }) {
                    "hunter".to_string()
                } else {
                    "hop-missed".to_string()
                };
                notes.push(format!("({spot_x},{spot_z}){why}+{dist}"));
            }
            let degrading = fallback.is_some();
            bot.log(&format!(
                "no {} reachable under {SAME_TIER_HOP_TILES} tiles{}: {} disqualified [{}], {far} beyond range",
                top.product,
                if degrading { ", degrading a tier" } else { "" },
                notes.len(),
                notes.join(" "),
            ));
            // Anti-famine: every in-range spot banned (none stumped, gated
            // or hunted — the tier is not depleted, just blacklisted) means
            // a transient storm poisoned the clocks. Retry the oldest ban
            // instead of degrading a tier dumbly; a still-dead spot re-bans
            // on the next attempt, one release per repick so no hot loop.
            if let Some(oldest) = starved_oldest(notes.len(), &banned_notes) {
                banned.remove(&oldest);
                bot.log(&format!(
                    "tier starved by bans: {} {} banned in range, releasing oldest ({},{})",
                    banned_notes.len(),
                    top.product,
                    oldest.0,
                    oldest.1,
                ));
                return pick_tree_spot(bot, tiers, bank, level, &mut *banned);
            }
        }
    }
    if saw_paid_only {
        bot.log("tree spots only across toll fence");
    }
    if saw_f2p_only {
        bot.log("tree spots only outside F2P");
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

/// Server reach is Chebyshev: the 8 neighbouring tiles count as adjacent.
/// A chop refusal from there means a wall/fence/gate stands between us and
/// the tree (or a fresh stump), never a walk that fell short.
pub(crate) fn adjacent_to(bot_x: i32, bot_z: i32, spot_x: i32, spot_z: i32) -> bool {
    (bot_x - spot_x).abs().max((bot_z - spot_z).abs()) <= 1
}

/// Adjacent direct refusals on a standing tree before it is condemned: the
/// first refusal rides the short ban (a transient storm or a gate mid-swing
/// refuses once), the repeat proves permanence (wall/fence). A walk-leg
/// failure never counts as a strike, however close the walk died.
pub(crate) const DIRECT_REFUSALS_TO_CONDEMN: u8 = 2;

/// Ban for a failed tree stand. `direct` is true only when the server
/// refused the chop click itself ("can't reach that"); walk-leg failures
/// (never arrived, leg timeout, step refused far away) pass false and can
/// never return the structural ban. Structural (240k) needs a repeated
/// adjacent direct refusal of a standing tree; everything else is
/// transient (a crowd, a gate mid-swing, a walk that fell short) and rides
/// the short ban (Gone, 250t) so the spot is re-tested at expiry instead
/// of sitting out half the session. A stump is depletion (Gone).
pub(crate) fn refusal_ban_ticks(adjacent: bool, stump: bool, direct: bool, prior_strikes: u8) -> u64 {
    if stump {
        return GONE_BAN_TICKS;
    }
    if direct && adjacent && prior_strikes + 1 >= DIRECT_REFUSALS_TO_CONDEMN {
        return STRUCTURAL_BAN_TICKS;
    }
    GONE_BAN_TICKS
}

/// Oldest banned spot to retry when a tier starves: Some only when every
/// in-range spot seen is banned (100% — none stumped, gated or hunted).
/// Anything else means the tier is genuinely unavailable, not blacklisted.
pub(crate) fn starved_oldest(total_seen: usize, banned: &[((i32, i32), u64)]) -> Option<(i32, i32)> {
    if total_seen == 0 || banned.len() != total_seen {
        return None;
    }
    banned.iter().min_by_key(|(_, until)| *until).map(|(pos, _)| *pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;
    use std::collections::HashMap;

    /// Refusal rule: orthogonal AND diagonal neighbours are adjacent
    /// (Chebyshev reach), two tiles off is not.
    #[test]
    fn adjacent_to_covers_chebyshev_reach() {
        assert!(adjacent_to(10, 10, 10, 10), "same tile");
        assert!(adjacent_to(10, 10, 11, 10), "orthogonal");
        assert!(adjacent_to(10, 10, 11, 11), "diagonal still adjacent");
        assert!(!adjacent_to(10, 10, 12, 10), "two tiles off walks short");
        assert!(!adjacent_to(10, 10, 11, 12), "two tiles off walks short");
    }

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
        let hopped = pick_tree_spot(&bot, &tiers, &bank, 0, &mut HashMap::new());
        let Some((_, hop_x, _)) = hopped else {
            panic!("hop should find a same-side tree, got None");
        };
        assert!(hop_x < 2920, "hop stays on the bot side of the fence, got x={hop_x}");
        bot.allow_paid = true;
        let some = pick_tree_spot(&bot, &tiers, &bank, 0, &mut HashMap::new());
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
        let mut banned: HashMap<(i32, i32), u64> = [
            (3053, 3271),
            (3041, 3319),
            (3017, 3308),
            (3184, 3226),
            (3146, 3254),
            (3151, 3230),
            (3165, 3219),
        ]
        .into_iter()
        .map(|pos| (pos, u64::MAX))
        .collect();
        let picked = pick_tree_spot(&bot, &tiers, &bank, 63, &mut banned);
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("hop should find the next yew cluster, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier, not willow");
        assert_eq!((spot_x, spot_z), (2996, 3311), "nearest live yew cluster, got: ({spot_x},{spot_z})");
    }

    /// Starvation: every yew in range banned (a transient storm poisoned
    /// the clocks) retries the oldest ban instead of degrading to willow.
    /// Staggered clocks prove the oldest goes first, and its entry leaves
    /// the map; a still-dead spot re-bans on the next attempt.
    #[test]
    fn starved_tier_releases_oldest_ban() {
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
        let mut banned: HashMap<(i32, i32), u64> = [
            ((3053, 3271), 1000u64),
            ((3041, 3319), 2001),
            ((3017, 3308), 2002),
            ((3184, 3226), 2003),
            ((3146, 3254), 2004),
            ((3151, 3230), 2005),
            ((3165, 3219), 2006),
            ((2996, 3311), 2007),
            ((2940, 3232), 2008),
            ((2933, 3233), 2009),
            ((2934, 3225), 2010),
            ((2935, 3229), 2011),
        ]
        .into_iter()
        .collect();
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned);
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("starved tier should retry the oldest ban, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier retried, not willow, got: {}", tier.product);
        assert_eq!((spot_x, spot_z), (3053, 3271), "oldest ban retried first, got: ({spot_x},{spot_z})");
        assert!(!banned.contains_key(&(3053, 3271)), "released entry leaves the map");
        assert_eq!(banned.len(), 11, "one release per repick, got: {}", banned.len());
    }

    /// Degrade still happens when the tier is genuinely unavailable: one
    /// in-range yew across a toll fence (not banned) breaks unanimity, so
    /// no release fires and the pick settles for willow next door.
    #[test]
    fn toll_breaks_unanimity_and_degrades() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3053;
        bot.world.z = 3271;
        bot.allow_wild = true;
        bot.allow_paid = false;
        bot.toll_gates.insert((3000, 3271, 0));
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        // Every in-range yew banned except (2996,3311), which sits across
        // the fence: its note reads "toll", not "banned", so no release.
        let mut banned: HashMap<(i32, i32), u64> = [
            (3053, 3271),
            (3041, 3319),
            (3017, 3308),
            (3184, 3226),
            (3146, 3254),
            (3151, 3230),
            (3165, 3219),
            (2940, 3232),
            (2933, 3233),
            (2934, 3225),
            (2935, 3229),
        ]
        .into_iter()
        .map(|pos| (pos, u64::MAX))
        .collect();
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned);
        let Some((tier, _, _)) = picked else {
            panic!("degrade should find a willow, got None");
        };
        assert_eq!(tier.product, "willow_logs", "fenced tier degrades, got: {}", tier.product);
        assert_eq!(banned.len(), 11, "no release without unanimity, got: {}", banned.len());
    }

    /// Ban rule: transit failures (never arrived, leg timeout, step refused
    /// far away) ride the short ban (250t, re-tested at expiry) even with
    /// a full strike history — only a repeated ADJACENT direct refusal
    /// condemns. A stump is depletion (Gone), a refusal from afar is a
    /// walk that fell short.
    #[test]
    fn direct_refusal_needs_a_repeat_to_condemn() {
        assert_eq!(refusal_ban_ticks(false, false, false, 5), GONE_BAN_TICKS, "far walk retries short");
        assert_eq!(refusal_ban_ticks(true, false, false, 5), GONE_BAN_TICKS, "walk failure retries short");
        assert_eq!(refusal_ban_ticks(true, false, true, 0), GONE_BAN_TICKS, "first refusal retries short");
        assert_eq!(refusal_ban_ticks(true, false, true, 1), STRUCTURAL_BAN_TICKS, "repeat condemns");
        assert_eq!(
            refusal_ban_ticks(false, false, true, 1),
            GONE_BAN_TICKS,
            "afar click is a short walk, not a walled stand"
        );
        assert_eq!(
            refusal_ban_ticks(true, true, true, 1),
            GONE_BAN_TICKS,
            "stump is depletion even when adjacent"
        );
    }

    /// Release rule: only a full ban sweep (100% of spots seen) releases
    /// the oldest; one stump, gate or hunter means genuinely unavailable.
    #[test]
    fn starved_oldest_needs_full_ban_sweep() {
        assert_eq!(starved_oldest(0, &[]), None, "nothing seen, nothing released");
        assert_eq!(starved_oldest(2, &[((1, 1), 50)]), None, "mixed stump+ban never releases");
        assert_eq!(
            starved_oldest(2, &[((1, 1), 50), ((2, 2), 30)]),
            Some((2, 2)),
            "oldest first, got: {:?}",
            starved_oldest(2, &[((1, 1), 50), ((2, 2), 30)])
        );
    }

    /// F2P filter: travel plans F2P-only, so an out-of-F2P spot is never a
    /// pick (live t01-4572: NoRoute OutsideF2p to a willow at (2906,3385)).
    /// The same query without a wired world keeps returning the spot: no
    /// world means no bounds, never silent filtering.
    #[test]
    fn f2p_filter_skips_outside_spots() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let data = root.join("LifeScape/gamedata/data");
        let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
        let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = lifescape_nav::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        assert!(!world.in_f2p(2862, 3385), "dossier spot is outside F2P");
        assert!(!world.in_f2p(2906, 3385), "dossier leg dest is outside F2P");
        assert!(world.in_f2p(3091, 3242), "Draynor bank booth is inside F2P");
        let bank = lifescape_gamedata::banks::Bank {
            x: 2860,
            z: 3380,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        // Every tree spawn in both pools (near-bank 150 and same-tier
        // hop 200 around the bot) banned except the out-of-F2P one, so
        // the unfiltered pick must land on it and the filtered pick must not.
        // Tier-agnostic: the dossier tile's tier is whatever it is.
        let mut tree_names = std::collections::HashSet::new();
        for tier in &tiers {
            for tree in &tier.trees {
                tree_names.insert(tree.clone());
            }
        }
        let mut banned: HashMap<(i32, i32), u64> = HashMap::new();
        for entry in lifescape_gamedata::all_locs() {
            let name = entry.name.clone().unwrap_or_default().to_lowercase();
            if !tree_names.contains(&name) {
                continue;
            }
            for spawn in &entry.spawns {
                if spawn.level == 0 && (spawn.x - 2862).abs() + (spawn.z - 3384).abs() <= 200 {
                    let pos = (spawn.x as i32, spawn.z as i32);
                    if pos != (2862, 3385) {
                        banned.insert(pos, u64::MAX);
                    }
                }
            }
        }
        let mut plain = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut plain);
        plain.world.level = 0;
        plain.world.x = 2862;
        plain.world.z = 3384;
        plain.allow_paid = true;
        plain.allow_wild = true;
        let picked = pick_tree_spot(&plain, &tiers, &bank, 30, &mut banned);
        let Some((_, spot_x, spot_z)) = picked else {
            panic!("unfiltered pick should land on the only open spot, got None");
        };
        assert_eq!((spot_x, spot_z), (2862, 3385), "unfiltered pick lands on it, got: ({spot_x},{spot_z})");
        let mut wired = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut wired);
        wired.world.level = 0;
        wired.world.x = 2862;
        wired.world.z = 3384;
        wired.allow_paid = true;
        wired.allow_wild = true;
        wired.nav_world = Some(world);
        let filtered = pick_tree_spot(&wired, &tiers, &bank, 30, &mut banned);
        let outside = filtered.as_ref().is_some_and(|(_, found_x, found_z)| (*found_x, *found_z) == (2862, 3385));
        assert!(!outside, "filtered pick never returns the F2P-excluded spot, got: {filtered:?}");
    }
}
