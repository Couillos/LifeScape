//! Woodcutter spot picking: tiers, spawns, liveness, toll fence and
//! hunter filters. Pure selection (no walking): the gather loop walks to
//! whatever this returns. Split from `skill_woodcutting` (file-size rule).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{GONE_BAN_TICKS, STRUCTURAL_BAN_TICKS, UNREACHABLE_BAN_TICKS};
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

/// Live footprint (width, length) of the loc on this tile. The server
/// reaches shape-10 locs orthogonally to their footprint (`ReachStrategy`
/// `reachRectangle1`, engine `routefinder/ReachStrategy.ts`), never
/// diagonally to the anchor tile: yews and oaks are 3x3, most trees 2x2
/// (`gametypes.json` loc rows). A diagonal-to-anchor stand touches no
/// footprint edge and the click always refuses — live 6/6: (3150,3229)
/// fails on (3151,3230) while (3151,3229) cuts, (3183,3225) fails on
/// (3184,3226) while (3185,3225) cuts, (3164,3220) cuts (3165,3219).
/// Unknown loc reads as 1x1 (old behaviour, engine arbitrates on click).
pub(crate) fn spot_footprint(bot: &Bot, spot_x: i32, spot_z: i32) -> (i64, i64) {
    let footprint = bot.world.loc_at(spot_x, spot_z, bot.world.level).and_then(|(loc, _, angle)| {
        knowledge::loc_by_id(usize::from(loc)).map(|entry| {
            if angle % 2 == 1 {
                (entry.length, entry.width)
            } else {
                (entry.width, entry.length)
            }
        })
    });
    footprint.unwrap_or((1, 1))
}

/// True when the stand is orthogonally adjacent to the loc footprint, i.e.
/// the server can reach it: same geometry as `reachRectangle1` minus the
/// wall-flag checks the engine arbitrates on click. Chebyshev-to-anchor is
/// NOT the rule (a 3x3 corner stand is "adjacent" but inoperable).
pub(crate) fn operable_stand(bot_x: i32, bot_z: i32, spot_x: i32, spot_z: i32, width: i64, length: i64) -> bool {
    let (anchor_x, anchor_z) = (i64::from(spot_x), i64::from(spot_z));
    let (stand_x, stand_z) = (i64::from(bot_x), i64::from(bot_z));
    (stand_x == anchor_x - 1 && stand_z >= anchor_z && stand_z < anchor_z + length)
        || (stand_x == anchor_x + width && stand_z >= anchor_z && stand_z < anchor_z + length)
        || (stand_z == anchor_z - 1 && stand_x >= anchor_x && stand_x < anchor_x + width)
        || (stand_z == anchor_z + length && stand_x >= anchor_x && stand_x < anchor_x + width)
}

/// One stand per footprint side (side middles): the click rotation tries
/// these before condemning the tree. Sorted nearest-first by the caller.
pub(crate) fn operable_ring(spot_x: i32, spot_z: i32, width: i64, length: i64) -> Vec<(i32, i32)> {
    let mut ring = vec![
        (spot_x - 1, spot_z + (length as i32) / 2),
        (spot_x + width as i32, spot_z + (length as i32) / 2),
        (spot_x + (width as i32) / 2, spot_z - 1),
        (spot_x + (width as i32) / 2, spot_z + length as i32),
    ];
    ring.sort();
    ring.dedup();
    ring
}

/// Ban for a failed walk to a stand. Planner-proven `NoRoute`
/// (`Error::Travel`, disconnected from here) bans 30k: re-planning the
/// same walk from the same tile is provably futile, so the spot sits out
/// ~30 min instead of burning a full plan every repick. Every other walk
/// failure (step refused mid-leg, leg timeout, door mid-swing) rides the
/// short ban (250t) and is re-tested at expiry.
pub(crate) fn walk_fail_ban(planner_proved: bool) -> u64 {
    if planner_proved {
        UNREACHABLE_BAN_TICKS
    } else {
        GONE_BAN_TICKS
    }
}

pub(crate) fn pick_tree_spot(
    bot: &Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &mut HashMap<(i32, i32), u64>,
    except: Option<(i32, i32)>,
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
            if Some((spot_x, spot_z)) == except {
                continue;
            }
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
            if Some((spot_x, spot_z)) == except {
                continue;
            }
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
                let why = if Some((spot_x, spot_z)) == except {
                    "noroute".to_string()
                } else if let Some(until) = banned.get(&(spot_x, spot_z)) {
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
            // A planner-proven dead spot (`except`, NoRoute from here)
            // breaks unanimity above ("noroute", never released): retrying
            // it from the same tile is provably futile until the bot moves.
            if let Some(oldest) = starved_oldest(notes.len(), &banned_notes) {
                banned.remove(&oldest);
                bot.log(&format!(
                    "tier starved by bans: {} {} banned in range, releasing oldest ({},{})",
                    banned_notes.len(),
                    top.product,
                    oldest.0,
                    oldest.1,
                ));
                return pick_tree_spot(bot, tiers, bank, level, &mut *banned, except);
            }
        }
    }
    if saw_paid_only {
        bot.log("tree spots only across toll fence");
    }
    if saw_f2p_only {
        bot.log("tree spots only outside F2P");
    }
    // Explicit downgrade: name both tiers so a willow interlude on a yew
    // account reads as a decision with a reason (the disqualified line
    // above), never a silent slide. `fallback` is the highest degraded
    // tier with a live spot, i.e. one tier below the best when it qualifies.
    if let (Some(top), Some((ref down, _, _))) = (best, fallback.as_ref()) {
        if down.level < top.level {
            bot.log(&format!("degrading {}->{} for this repick (best tier unreachable, reasons above)", top.product, down.product));
        }
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

/// Operable direct refusals on a standing tree before it is condemned: the
/// first refusal rides the short ban (a transient storm or a gate mid-swing
/// refuses once), the repeat proves permanence (wall/fence). A walk-leg
/// failure never counts as a strike, however close the walk died. Geometry
/// is footprint-orthogonal (`operable_stand`), never Chebyshev: a refused
/// diagonal-to-anchor corner is a bad stand, not a walled tree.
pub(crate) const DIRECT_REFUSALS_TO_CONDEMN: u8 = 2;

/// Ban for a failed tree stand. `direct` is true only when the server
/// refused the chop click itself ("can't reach that") from a
/// footprint-operable stand; walk-leg failures (never arrived, leg timeout,
/// step refused far away, inoperable stand) pass false and can never return
/// the structural ban. Structural (240k) needs a repeated operable direct
/// refusal of a standing tree; everything else is transient (a crowd, a
/// gate mid-swing, a walk that fell short) and rides the short ban (Gone,
/// 250t) so the spot is re-tested at expiry instead of sitting out half the
/// session. A stump is depletion (Gone).
pub(crate) fn refusal_ban_ticks(operable: bool, stump: bool, direct: bool, prior_strikes: u8) -> u64 {
    if stump {
        return GONE_BAN_TICKS;
    }
    if direct && operable && prior_strikes + 1 >= DIRECT_REFUSALS_TO_CONDEMN {
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
        let hopped = pick_tree_spot(&bot, &tiers, &bank, 0, &mut HashMap::new(), None);
        let Some((_, hop_x, _)) = hopped else {
            panic!("hop should find a same-side tree, got None");
        };
        assert!(hop_x < 2920, "hop stays on the bot side of the fence, got x={hop_x}");
        bot.allow_paid = true;
        let some = pick_tree_spot(&bot, &tiers, &bank, 0, &mut HashMap::new(), None);
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
        let picked = pick_tree_spot(&bot, &tiers, &bank, 63, &mut banned, None);
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
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None);
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
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None);
        let Some((tier, _, _)) = picked else {
            panic!("degrade should find a willow, got None");
        };
        assert_eq!(tier.product, "willow_logs", "fenced tier degrades, got: {}", tier.product);
        assert_eq!(banned.len(), 11, "no release without unanimity, got: {}", banned.len());
    }

    /// Strike rule: only a refusal from a footprint-operable stand counts
    /// toward condemning (a corner refusal is a bad stand, retried short).
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
        let picked = pick_tree_spot(&plain, &tiers, &bank, 30, &mut banned, None);
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
        let filtered = pick_tree_spot(&wired, &tiers, &bank, 30, &mut banned, None);
        let outside = filtered.as_ref().is_some_and(|(_, found_x, found_z)| (*found_x, *found_z) == (2862, 3385));
        assert!(!outside, "filtered pick never returns the F2P-excluded spot, got: {filtered:?}");
    }

    /// Operable-stand geometry mirrors the server (`ReachStrategy`
    /// `reachRectangle1`): orthogonal to the footprint, never diagonal to
    /// the anchor. Live 6/6 fixtures: a yew is 3x3, so (3151,3229) cuts
    /// (3151,3230) but the diagonal corner (3150,3229) refuses; (3185,3225)
    /// cuts (3184,3226) but (3183,3225) refuses; (3164,3220) and
    /// (3166,3218) both cut (3165,3219) (west side, inside the footprint
    /// edge range).
    #[test]
    fn operable_stand_matches_server_reach() {
        // Yew 3x3 at (3151,3230): south side works, west corner refuses.
        assert!(operable_stand(3151, 3229, 3151, 3230, 3, 3), "south side cuts");
        assert!(!operable_stand(3150, 3229, 3151, 3230, 3, 3), "diagonal corner refuses");
        // Yew 3x3 at (3184,3226): east-diagonal works, west-diagonal refuses.
        assert!(operable_stand(3185, 3225, 3184, 3226, 3, 3), "south-east diagonal works");
        assert!(!operable_stand(3183, 3225, 3184, 3226, 3, 3), "south-west corner refuses");
        // Yew 3x3 at (3165,3219): west side and south side both cut.
        assert!(operable_stand(3164, 3220, 3165, 3219, 3, 3), "west side cuts");
        assert!(operable_stand(3166, 3218, 3165, 3219, 3, 3), "south side cuts");
        // Willow 2x2: side middles cut, corners refuse, far tiles refuse.
        assert!(operable_stand(9, 10, 10, 10, 2, 2), "west side cuts");
        assert!(!operable_stand(9, 9, 10, 10, 2, 2), "corner refuses");
        assert!(!operable_stand(13, 11, 10, 10, 2, 2), "two tiles off refuses");
        // 1x1 keeps the old adjacency (all four orthogonals, no diagonals).
        assert!(operable_stand(5, 4, 5, 5, 1, 1), "south cuts");
        assert!(!operable_stand(4, 4, 5, 5, 1, 1), "diagonal refuses");
    }

    /// Ring: one stand per footprint side, deduped (a 3x3 yields 4
    /// distinct side middles, a 1x1 the four orthogonals).
    #[test]
    fn operable_ring_covers_four_sides() {
        let mut ring = operable_ring(3151, 3230, 3, 3);
        ring.sort();
        assert_eq!(ring, vec![(3150, 3231), (3152, 3229), (3152, 3233), (3154, 3231)], "side middles, got: {ring:?}");
        let mut single = operable_ring(5, 5, 1, 1);
        single.sort();
        assert_eq!(single, vec![(4, 5), (5, 4), (5, 6), (6, 5)], "four orthogonals, got: {single:?}");
    }

    /// Walk-failure bans: planner-proven NoRoute sits out 30k (re-planning
    /// from the same tile is futile), every transient walk failure rides
    /// the short ban and is re-tested at expiry.
    #[test]
    fn walk_fail_ban_is_proven_or_short() {
        assert_eq!(walk_fail_ban(true), UNREACHABLE_BAN_TICKS, "planner proof bans long");
        assert_eq!(walk_fail_ban(false), GONE_BAN_TICKS, "transient walk retries short");
    }

    /// Planner-proven dead spot (`except`): never picked, never
    /// starvation-released, breaks unanimity so the repick degrades to the
    /// highest CUTTABLE tier instead of hot-looping the dead yew. Near
    /// Draynor no maple qualifies (dump: maples live ~300 tiles off, past
    /// hop range), so yew->willow with oak/logs untouched: tiers are walked
    /// in order, never skipped when available, and the downgrade is logged.
    #[test]
    fn except_dead_spot_degrades_one_tier() {
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
        // Every in-range yew banned except the planner-dead one, which is
        // excluded instead of released: the repick must degrade to maple.
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
        let picked = pick_tree_spot(&bot, &tiers, &bank, 63, &mut banned, Some((2996, 3311)));
        let Some((tier, _, _)) = picked else {
            panic!("dead-spot repick should degrade one tier, got None");
        };
        assert_eq!(tier.product, "willow_logs", "highest cuttable tier below yew, got: {}", tier.product);
        assert!(tier.product != "oak_logs" && tier.product != "logs", "never crashes to the bottom, got: {}", tier.product);
        assert_eq!(banned.len(), 11, "no release without unanimity, got: {}", banned.len());
    }
}
