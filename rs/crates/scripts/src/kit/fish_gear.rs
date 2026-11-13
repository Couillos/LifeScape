//! Fish gear: which tier a run fishes and the gear it needs, executor
//! side. The brain names the fish (`fishing raw_trout`): the run fishes
//! that method only (trout/salmon → fly fishing rod + feathers, pike →
//! rod + bait, shrimp → net…), takes tool and bait from the pack or the
//! bank, and reports what is missing by exact name
//! (`blocked{fly_fishing_rod, feather}`). Never a shop buy: feathers come
//! from chickens or the market, tools and bait through the brain's own
//! obtain legs (live 2026-11-13: b3gybcev bought 24 feathers itself,
//! b8jxrdd2 bait-fished pike on a trout order).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::{banked_usable, obj_id, pack_total};
use crate::ScriptError;

/// Bait below this re-stocks from the bank (or blocks).
pub const BAIT_MIN: u32 = 20;
/// Bait taken out of the bank per restock.
pub const BAIT_WANT: u32 = 200;

/// The tier catching `fish` and the level it needs (the second product
/// of a tier needs its `second_level`). Unknown fish: Usage.
pub fn target_fish_tier(fish: &str) -> Result<(knowledge::fishing::FishTier, i64), ScriptError> {
    for tier in knowledge::fishing::all_fish_tiers() {
        if let Some(index) = tier.products.iter().position(|product| product.eq_ignore_ascii_case(fish)) {
            let need = if index == 0 { tier.level } else { tier.second_level.max(tier.level) };
            return Ok((tier, need));
        }
    }
    Err(ScriptError::Usage(format!("no F2P fishing method catches {fish:?}")))
}

/// The brain's fish as a tier, level-gated: `blocked{level:fishing:N}`
/// below it (a typed gap the brain trains, never a Usage retry).
pub fn ordered_tier(fish: &str, level: i64) -> Result<knowledge::fishing::FishTier, ScriptError> {
    let (tier, need) = target_fish_tier(fish)?;
    if level < need {
        return Err(crate::kit::cycle::blocked_level("fishing", need));
    }
    Ok(tier)
}

/// Units of `name` in the pack and the known bank.
fn held_anywhere(bot: &Bot, name: &str) -> (u32, u32) {
    match obj_id(bot, name) {
        Ok(id) => (pack_total(bot, id), banked_usable(bot, id)),
        Err(_) => (0, 0),
    }
}

/// Exact names the tier still lacks, counting pack and known bank: the
/// tool, then the bait (fewer than `BAIT_MIN` anywhere).
pub fn gear_missing(bot: &Bot, tier: &knowledge::fishing::FishTier) -> Vec<String> {
    let mut missing = Vec::new();
    let (tool_pack, tool_bank) = held_anywhere(bot, tier.tool);
    if tool_pack + tool_bank == 0 {
        missing.push(tier.tool.to_string());
    }
    if let Some(bait) = tier.bait {
        let (bait_pack, bait_bank) = held_anywhere(bot, bait);
        if bait_pack + bait_bank < BAIT_MIN {
            missing.push(bait.to_string());
        }
    }
    missing
}

/// Gear pass: 0 = tool and bait in the pack, 1 = some of it in the bank,
/// `None` = something missing everywhere.
fn tier_pass(bot: &Bot, tier: &knowledge::fishing::FishTier) -> Option<u8> {
    if !gear_missing(bot, tier).is_empty() {
        return None;
    }
    let tool_ready = held_anywhere(bot, tier.tool).0 > 0;
    let bait_ready = tier.bait.is_none_or(|bait| held_anywhere(bot, bait).0 >= BAIT_MIN);
    Some(if tool_ready && bait_ready { 0 } else { 1 })
}

/// Manual runs (no ordered fish): the best tier geared from pack or bank;
/// nothing geared blocks on the best tier's exact gaps.
pub fn select_fish_tier(bot: &Bot, level: i64) -> Result<knowledge::fishing::FishTier, ScriptError> {
    let mut best: Option<(u8, knowledge::fishing::FishTier)> = None;
    for tier in knowledge::fishing::affordable_tiers(level) {
        if let Some(pass) = tier_pass(bot, &tier) {
            if best.as_ref().is_none_or(|(known_pass, known)| (pass, -tier.level) < (*known_pass, -known.level)) {
                best = Some((pass, tier));
            }
        }
    }
    if let Some((_, tier)) = best {
        return Ok(tier);
    }
    let mut by_level = knowledge::fishing::affordable_tiers(level);
    by_level.sort_by_key(|tier| tier.level);
    let cheapest = by_level.into_iter().next().ok_or_else(|| ScriptError::Blocked { missing: vec!["net".to_string()] })?;
    Err(ScriptError::Blocked { missing: gear_missing(bot, &cheapest) })
}

/// Tiers to try in order. An ordered fish is the only tier, whatever the
/// gear (the work step blocks on its exact gaps). Manual runs: geared
/// tiers first (pack before bank, best level), bait-banned ones skipped.
pub fn ordered_fish_tiers(
    bot: &Bot,
    level: i64,
    bait_ban_until: u64,
    now: u64,
    target: Option<&knowledge::fishing::FishTier>,
) -> Vec<knowledge::fishing::FishTier> {
    if let Some(tier) = target {
        return vec![tier.clone()];
    }
    let mut geared: Vec<(u8, knowledge::fishing::FishTier)> = Vec::new();
    for tier in knowledge::fishing::affordable_tiers(level) {
        if tier.bait.is_some() && now < bait_ban_until {
            continue;
        }
        if let Some(pass) = tier_pass(bot, &tier) {
            geared.push((pass, tier));
        }
    }
    geared.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.level.cmp(&left.1.level)));
    geared.into_iter().map(|(_, tier)| tier).collect()
}

/// Tool and bait in the pack before a spot: banked ones are withdrawn
/// (walking to a far bank when the known view holds them). A tool missing
/// everywhere blocks with every exact gap; bait short after the bank is
/// `Ok(false)` (the caller blocks or, on manual runs, drops the tier).
pub fn ensure_gear(bot: &mut Bot, tier: &knowledge::fishing::FishTier, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    let tool = obj_id(bot, tier.tool)?;
    if pack_total(bot, tool) == 0 && crate::kit::withdraw::bank_withdraw_items(bot, bank, &[tool], 1)? > 0 {
        bot.log(&format!("ok {} withdrawn from bank", tier.tool));
    }
    if pack_total(bot, tool) == 0 {
        let mut missing = vec![tier.tool.to_string()];
        missing.extend(gear_missing(bot, tier).into_iter().filter(|name| name != tier.tool));
        return Err(ScriptError::Blocked { missing });
    }
    restock_bait(bot, tier, bank)
}

/// Bait back to `BAIT_WANT` from the bank only. True when at least
/// `BAIT_MIN` is carried afterwards (always true for unbaited tiers).
pub fn restock_bait(bot: &mut Bot, tier: &knowledge::fishing::FishTier, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    let Some(bait) = tier.bait else { return Ok(true) };
    let bait_id = obj_id(bot, bait)?;
    if pack_total(bot, bait_id) >= BAIT_MIN {
        return Ok(true);
    }
    let need = BAIT_WANT.saturating_sub(pack_total(bot, bait_id));
    if crate::kit::withdraw::bank_withdraw_items(bot, bank, &[bait_id], need)? > 0 {
        bot.log(&format!("ok {bait} withdrawn from bank"));
    }
    Ok(pack_total(bot, bait_id) >= BAIT_MIN)
}

#[cfg(test)]
mod tests {
    use super::{gear_missing, ordered_fish_tiers, ordered_tier, select_fish_tier, target_fish_tier};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn gear_bot(pack: &[(&str, u32)], bank: &[(&str, u32)]) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let resolve = |bot: &Bot, items: &[(&str, u32)]| -> Vec<(u16, u32)> { items.iter().map(|(name, count)| (obj_id(bot, name).expect("obj"), *count)).collect() };
        let pack_ids = resolve(&bot, pack);
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv");
        bot.world.inv.insert(com, pack_ids.into_iter().enumerate().map(|(slot, entry)| (slot as u8, entry)).collect());
        let bank_ids = resolve(&bot, bank);
        let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank_main:inv");
        bot.world.seed_closed_inv(bank_com, &bank_ids.into_iter().collect());
        bot
    }

    /// Each fish maps to its method and level (salmon is the fly tier's
    /// second catch, level 30).
    #[test]
    fn fish_maps_to_method_and_level() {
        let (trout, need) = target_fish_tier("raw_trout").expect("trout");
        assert_eq!((trout.tool, trout.bait, need), ("fly_fishing_rod", Some("feather"), 20));
        let (_, salmon) = target_fish_tier("raw_salmon").expect("salmon");
        assert_eq!(salmon, 30);
        let (pike, _) = target_fish_tier("raw_pike").expect("pike");
        assert_eq!((pike.tool, pike.bait), ("fishing_rod", Some("fishing_bait")));
        assert_eq!(target_fish_tier("raw_shrimp").expect("shrimp").0.tool, "net");
        assert!(matches!(ordered_tier("raw_salmon", 25), Err(crate::ScriptError::Blocked { .. })), "level gap blocks");
        assert!(matches!(target_fish_tier("raw_whale"), Err(crate::ScriptError::Usage(_))));
    }

    /// b8jxrdd2 (fishing 50, rod + bait carried) on a trout order: the
    /// order is the only tier, never pike; its gaps are named exactly.
    #[test]
    fn ordered_trout_never_fishes_pike_and_names_its_gaps() {
        let bot = gear_bot(&[("fishing_rod", 1), ("fishing_bait", 200)], &[]);
        let trout = ordered_tier("raw_trout", 50).expect("trout at 50");
        let tiers = ordered_fish_tiers(&bot, 50, 0, 0, Some(&trout));
        assert_eq!(tiers.iter().map(|tier| tier.tool).collect::<Vec<_>>(), vec!["fly_fishing_rod"], "trout only");
        assert_eq!(gear_missing(&bot, &trout), vec!["fly_fishing_rod".to_string(), "feather".to_string()]);
        let banked = gear_bot(&[], &[("fly_fishing_rod", 1), ("feather", 500)]);
        assert!(gear_missing(&banked, &trout).is_empty(), "banked gear counts");
    }

    /// Manual runs: geared tiers only, pack before bank; nothing geared
    /// blocks on the cheapest tier's exact gaps (no coins, no shop).
    #[test]
    fn manual_runs_pick_geared_tiers_without_shops() {
        let net = gear_bot(&[("net", 1)], &[]);
        assert_eq!(select_fish_tier(&net, 5).expect("net fishes").tool, "net");
        let rod = gear_bot(&[("fishing_rod", 1), ("fishing_bait", 200)], &[]);
        assert_eq!(select_fish_tier(&rod, 5).expect("rod fishes").tool, "fishing_rod");
        let broke = gear_bot(&[("coins", 1000)], &[]);
        match select_fish_tier(&broke, 5) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["net".to_string()]),
            other => panic!("coins never buy gear, got {other:?}"),
        }
        let bait_banned: Vec<&str> = ordered_fish_tiers(&rod, 5, 1000, 10, None).iter().map(|tier| tier.tool).collect();
        assert!(!bait_banned.contains(&"fishing_rod"), "bait-banned tiers skipped: {bait_banned:?}");
    }
}
