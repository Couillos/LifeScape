//! Tools: shared tool operations. The rank itself (`ToolSpec`,
//! best-first order, level gates through `usable`) plus reattach and
//! banked-restore, used by every gather skill. Bank restores go through
//! one function: withdraw the best banked tool, else the banked
//! handle+head, never a second tool in the pack.

#![forbid(unsafe_code)]

pub use crate::kit::bans::BanSet;

use std::collections::{HashMap, HashSet};

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, live_bank_booth};
use crate::kit::pack::{obj_id, obj_price, pack_ids, pack_slot, pack_total};
use crate::kit::shop::buy_stock;
use crate::ScriptError;

/// Typed missing-tool error: the brain files it as an obtain sub-task.
/// The name is the cheapest buyable of the family (`bronze_axe`,
/// `bronze_pickaxe`, `net`), since that is what the task needs funded.
pub fn blocked_tool(name: &str) -> ScriptError {
    ScriptError::Blocked { missing: vec![name.to_string()] }
}

/// Free tool reclaim (no shop, no hunt): pack reattach, then the best
/// banked tool. True when a tool is held afterwards. Constrained tasks
/// (`--no-shop`) use only this; anything still missing blocks.
pub fn reclaim_free(
    bot: &mut Bot,
    bank: &lifescape_gamedata::banks::Bank,
    spec: &ToolSpec,
    handle_name: &str,
    head_suffix: &str,
) -> Result<bool, ScriptError> {
    if spec.best_held(bot).is_some() {
        return Ok(true);
    }
    if reattach_tool(bot, handle_name, head_suffix, spec)? {
        bot.log("ok tool reattached from pack, no bootstrap needed");
    }
    if spec.best_held(bot).is_none() {
        let _ = restore_from_bank(bot, bank, spec, handle_name, head_suffix)?;
    }
    Ok(spec.best_held(bot).is_some())
}

/// Walk budget to fetch a banked tool from across the map (one bank
/// trip transit slice x 2).
pub const TOOL_BANK_WALK_TICKS: u64 = 3000;

/// True when the last known bank view (open or closed) holds a usable
/// (unlocked) tool of `spec`.
pub fn banked_tool_held(bot: &Bot, spec: &ToolSpec) -> bool {
    spec.known_ids(bot).into_iter().any(|id| crate::kit::pack::banked_usable(bot, id) > 0)
}

/// Withdraw the best banked tool of a spec, no handle/head reassembly:
/// for single-item tools (fishing nets) there is nothing to reattach.
/// Returns true when a tool is held afterwards.
pub fn restore_banked_tool(
    bot: &mut Bot,
    bank: &lifescape_gamedata::banks::Bank,
    spec: &ToolSpec,
) -> Result<bool, ScriptError> {
    if spec.best_held(bot).is_some() {
        return Ok(true);
    }
    let far = |bot: &Bot| (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12;
    if far(bot) && banked_tool_held(bot, spec) {
        // A clean pack skips the bank trip (true no-op): the restore walks
        // itself when the bank view shows a usable tool (live 2026-11-13,
        // bdr53jee: banked shears, blocked x4 from across the map).
        crate::kit::withdraw::walk_to_bank_for(bot, bank, "a tool")?;
    }
    if far(bot) {
        bot.log(&format!("tool restore: bank at ({},{}) too far from {:?}", bank.x, bank.z, bot.position()));
        return Ok(false);
    }
    let Some((booth_x, booth_z, booth_loc)) = live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
        return Ok(false);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        return Ok(false);
    }
    let Some(main_com) = (bot.tables.com_id)("bank_main:inv") else {
        bot.close_modal()?;
        return Ok(false);
    };
    let slots = bot.world.inv.get(&main_com).cloned().unwrap_or_default();
    let known = spec.known_ids(bot);
    // One tool in the pack: withdraw the best banked one, never a second.
    let rank = |name: &str| spec.rank(name);
    if let Some((slot, held)) = best_banked_by_rank(&slots, &known, rank) {
        bot.inv_button(1, held, slot, main_com)?;
        bot.pump(10)?;
        bot.close_modal()?;
        bot.pump(5)?;
        if spec.best_held(bot).is_some() {
            bot.log("ok withdrew banked tool");
            return Ok(true);
        }
        return Ok(false);
    }
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(false)
}

/// Withdraw a banked tool (or its handle+head) and reattach it. Cheaper
/// than rebuying. Reopens the bank: call after a bank trip, or not at
/// all when the bank is further than a dozen tiles (booth business from
/// across the map fails instead of fanning walks at an untouchable
/// booth).
pub fn restore_from_bank(
    bot: &mut Bot,
    bank: &lifescape_gamedata::banks::Bank,
    spec: &ToolSpec,
    handle_name: &str,
    head_suffix: &str,
) -> Result<bool, ScriptError> {
    // Whole tools first (single open inside); pieces need a second open
    // only when nothing whole rests.
    if restore_banked_tool(bot, bank, spec)? {
        return Ok(true);
    }
    let Ok(handle) = obj_id(bot, handle_name) else {
        return Ok(false);
    };
    let heads: Vec<u16> = lifescape_gamedata::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with(head_suffix))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    if heads.is_empty() {
        return Ok(false);
    }
    let Some((booth_x, booth_z, booth_loc)) = live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
        return Ok(false);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        return Ok(false);
    }
    let Some(main_com) = (bot.tables.com_id)("bank_main:inv") else {
        bot.close_modal()?;
        return Ok(false);
    };
    let slots = bot.world.inv.get(&main_com).cloned().unwrap_or_default();
    // Pieces only: whole tools were already attempted by the delegation
    // above (second open only when nothing whole rests).
    let handle_slot = slots.iter().find(|(_, (held, _))| *held == handle).map(|(slot, _)| *slot);
    let head_slot = slots.iter().find(|(_, (held, _))| heads.contains(held)).map(|(slot, _)| *slot);
    let (Some(handle_slot), Some(head_slot)) = (handle_slot, head_slot) else {
        bot.close_modal()?;
        bot.pump(5)?;
        return Ok(false);
    };
    let head = slots[&head_slot].0;
    bot.inv_button(1, handle, handle_slot, main_com)?;
    bot.pump(5)?;
    bot.inv_button(1, head, head_slot, main_com)?;
    bot.pump(10)?;
    bot.close_modal()?;
    bot.pump(5)?;
    if reattach_tool(bot, handle_name, head_suffix, spec)? {
        bot.log("ok restored tool from banked pieces");
        return Ok(true);
    }
    Ok(false)
}

pub fn recover_axe(bot: &mut Bot, coins: u16, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if AXE_SPEC.best_held(bot).is_some() {
        return Ok(true);
    }
    if reattach_tool(bot, AXE_HANDLE, AXE_HEAD_SUFFIX, &AXE_SPEC)? {
        return Ok(true);
    }
    if pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0 {
        bot.log("ok replacement axe bought");
        return Ok(true);
    }
    let keep = vec![coins];
    if bank_trip(bot, bank, &keep, 16u32.saturating_sub(pack_total(bot, coins)))? == 0 {
        return Ok(false);
    }
    if AXE_SPEC.best_held(bot).is_some() {
        bot.log("ok replacement axe restored");
        return Ok(true);
    }
    Ok(pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0)
}

/// Best banked tool per kit spec, plus tinderbox: the protected tool set
/// for the bank janitor (worker-side half of the planner triage).
pub fn cleanup_keep_names(bot: &Bot, banked: &std::collections::HashSet<u16>) -> Vec<String> {
    let mut keep = Vec::new();
    for spec in [&AXE_SPEC, &PICK_SPEC, &FISH_SPEC] {
        let mut best: Option<(usize, &str)> = None;
        for name in spec.order {
            let rank = spec.rank(name).unwrap_or(usize::MAX);
            let held = obj_id(bot, name).is_ok_and(|id| banked.contains(&id));
            if held && best.is_none_or(|(top, _)| rank < top) {
                best = Some((rank, name));
            }
        }
        if let Some((_, name)) = best {
            keep.push(name.to_string());
        }
    }
    if obj_id(bot, "tinderbox").is_ok_and(|id| banked.contains(&id)) {
        keep.push("tinderbox".to_string());
    }
    keep
}

// -- tool specs (best-first orders; the server works with the best carried
// tool, so rank alone decides which one to keep, restore or upgrade) --

/// Best axe first.
pub const AXE_ORDER: [&str; 7] =
    ["rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe"];
/// Axe handle item for reattaching.
pub const AXE_HANDLE: &str = "macro_hatchethandle";
/// Axe head suffix for reattaching.
pub const AXE_HEAD_SUFFIX: &str = "_hatchethead";
/// Shared axe rank: the server takes the best carried axe at any level,
/// so rank alone decides.
pub const AXE_SPEC: ToolSpec = ToolSpec { order: &AXE_ORDER };

/// Best pickaxe first.
pub const PICK_ORDER: [&str; 6] =
    ["rune_pickaxe", "adamant_pickaxe", "mithril_pickaxe", "steel_pickaxe", "iron_pickaxe", "bronze_pickaxe"];
/// Wield/mining level reqs (content pickaxes config; the obj dump carries
/// no params, so the six values live here).
pub const PICK_REQS: [(&str, i64); 6] =
    [("bronze_pickaxe", 0), ("iron_pickaxe", 0), ("steel_pickaxe", 6), ("mithril_pickaxe", 21), ("adamant_pickaxe", 31), ("rune_pickaxe", 41)];
/// Pickaxe handle item for reattaching.
pub const PICK_HANDLE: &str = "macro_pickaxehandle";
/// Pickaxe head suffix for reattaching.
pub const PICK_HEAD_SUFFIX: &str = "_pickaxehead";
/// Shared rank: best pickaxe first, level gates pass through `usable`
/// (`ToolSpec` decides, the tables carry no params).
pub const PICK_SPEC: ToolSpec = ToolSpec { order: &PICK_ORDER };

/// Best tool first, by max tier level: lobster 40, harpoon 35, rod 25,
/// fly 20, net 1 (the rod serves two tiers, its rank is the higher one).
/// Matches the old best-by-level pick on every held subset.
pub const FISH_ORDER: [&str; 5] =
    ["lobster_pot", "harpoon", "fishing_rod", "fly_fishing_rod", "net"];
/// Shared rank: best held tool wins, level gates pass through `usable`.
pub const FISH_SPEC: ToolSpec = ToolSpec { order: &FISH_ORDER };

/// Affordable pickaxe upgrade candidates, best-first, with the cheapest
/// price: better-ranked than held, level-usable by the caller, actually
/// in shop stock.
pub fn cheapest_pickaxe_upgrade(bot: &Bot, level: i64) -> (Vec<String>, i64) {
    // shortcut: only surface-reachable shops stock upgrades (Nurmof sits
    // down the dwarven-mine stairs), so usually just bronze; better ones
    // land as soon as stairs traversal does.
    PICK_SPEC.cheapest_upgrade(bot, |name| PICK_REQS.iter().find(|(tool, _)| tool == &name).is_some_and(|(_, req)| level >= *req))
}

/// Affordable fishing upgrade candidates, best-first, with the cheapest
/// price.
pub fn cheapest_fish_upgrade(bot: &Bot, level: i64) -> (Vec<String>, i64) {
    FISH_SPEC.cheapest_upgrade(bot, |name| {
        lifescape_gamedata::fishing::all_fish_tiers().iter().any(|tier| tier.tool == name && tier.level <= level)
    })
}

/// Pickaxe recovery: reattach from the pack, buy bronze when funded, else
/// bank-trip and restore the best banked pickaxe. Used mid-load when the
/// tool is gone; the run loop re-enters with the same bans.
pub fn recover_pickaxe(bot: &mut Bot, coins: u16, bank: &lifescape_gamedata::banks::Bank) -> Result<bool, ScriptError> {
    if PICK_SPEC.best_held(bot).is_some() {
        return Ok(true);
    }
    if reattach_tool(bot, PICK_HANDLE, PICK_HEAD_SUFFIX, &PICK_SPEC)? {
        return Ok(true);
    }
    let bronze_price = crate::kit::pack::obj_price("bronze_pickaxe") as u32;
    if pack_total(bot, coins) >= bronze_price && crate::kit::shop::buy_stock(bot, "bronze_pickaxe", 1)? > 0 {
        bot.log("ok replacement pickaxe bought");
        return Ok(true);
    }
    let Some(home) = crate::kit::bank::nearest_bank(bot).ok() else {
        return Ok(false);
    };
    let keep = vec![coins];
    if bank_trip(bot, &home, &keep, bronze_price.saturating_sub(pack_total(bot, coins)))? == 0 {
        let _ = bank;
        return Ok(false);
    }
    if PICK_SPEC.best_held(bot).is_none() && restore_from_bank(bot, &home, &PICK_SPEC, PICK_HANDLE, PICK_HEAD_SUFFIX)? {
        bot.log("ok replacement pickaxe restored from bank");
        return Ok(true);
    }
    if PICK_SPEC.best_held(bot).is_some() {
        bot.log("ok replacement pickaxe restored");
        return Ok(true);
    }
    Ok(pack_total(bot, coins) >= bronze_price && crate::kit::shop::buy_stock(bot, "bronze_pickaxe", 1)? > 0)
}


/// Best-first tool order (`AXE_ORDER`, `PICK_ORDER`, fishing tools by max
/// tier level): rank decides, shop stock gates, level gates pass through
/// `usable`. State reads only, never a walk or a buy.
pub struct ToolSpec {
    pub order: &'static [&'static str],
}

impl ToolSpec {
    /// Rank in `order` (0 = best). Unknown tools never rank.
    pub fn rank(&self, name: &str) -> Option<usize> {
        self.order.iter().position(|tool| *tool == name)
    }

    /// Best held tool, best-first: the pack or the worn right hand (the
    /// server accepts a wielded pickaxe or axe: pickaxe_checker.rs2,
    /// woodcut.rs2).
    pub fn best_held(&self, bot: &Bot) -> Option<String> {
        let mut held = pack_ids(bot);
        if let Some(worn_com) = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME) {
            if let Some(slots) = bot.world.inv.get(&worn_com) {
                held.extend(slots.values().map(|(id, _)| *id));
            }
        }
        self.order
            .iter()
            .find(|name| obj_id(bot, name).is_ok_and(|id| held.contains(&id)))
            .map(|name| name.to_string())
    }

    /// Every order id the tables know (for dirty-pack and sale filters:
    /// a banked tool type still counts as a tool, never as junk).
    pub fn known_ids(&self, bot: &Bot) -> HashSet<u16> {
        self.order.iter().filter_map(|name| obj_id(bot, name).ok()).collect()
    }

    /// Pack ids that survive the bank trip: the single best tool only (the
    /// server works with the best carried tool, so a second is dead weight
    /// that death would drop anyway).
    pub fn keep_ids(&self, bot: &Bot) -> Vec<u16> {
        self.best_held(bot).and_then(|name| obj_id(bot, &name).ok()).into_iter().collect()
    }

    /// Affordable upgrade cands, best-first, with the cheapest price:
    /// better-ranked than held, level-usable by the caller, actually in
    /// shop stock. One pass, never a loop.
    pub fn cheapest_upgrade(&self, bot: &Bot, usable: impl Fn(&str) -> bool) -> (Vec<String>, i64) {
        let current = self.best_held(bot).and_then(|name| self.rank(&name)).unwrap_or(self.order.len());
        let cands: Vec<String> = self.order[..current]
            .iter()
            .filter(|name| usable(name) && crate::kit::shop::shop_stock_keeper(bot, name).is_some())
            .map(|name| name.to_string())
            .collect();
        let cheapest = cands.iter().map(|name| obj_price(name)).min().unwrap_or(0);
        (cands, cheapest)
    }
}

/// Reattach a carried handle + head instead of rebuying (axe and pickaxe
/// loops share it). Pack ops only, no bank modal: resolve the handle, try
/// each carried head with `head_suffix`, verify through `spec` (a plain
/// ref, never a closure over `&Bot`, so no double-borrow). Ok(false) when
/// the handle or every head is missing: nothing sent.
pub fn reattach_tool(bot: &mut Bot, handle_name: &str, head_suffix: &str, spec: &ToolSpec) -> Result<bool, ScriptError> {
    let Ok(handle) = obj_id(bot, handle_name) else {
        return Ok(false);
    };
    if pack_total(bot, handle) == 0 {
        return Ok(false);
    }
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with(head_suffix))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    for head in heads {
        if pack_total(bot, head) == 0 {
            continue;
        }
        if use_handle_on_head(bot, handle, head, spec)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn use_handle_on_head(bot: &mut Bot, handle: u16, head: u16, spec: &ToolSpec) -> Result<bool, ScriptError> {
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    let (Ok(handle_slot), Ok(head_slot)) = (pack_slot(bot, handle), pack_slot(bot, head)) else {
        return Ok(false);
    };
    bot.op_held_u(handle, handle_slot, pack_com, head, head_slot, pack_com)?;
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if spec.best_held(bot).is_some() {
            bot.log("ok tool reattached");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Best-ranked banked tool slot (the pack holds one tool: an upgrade banks
/// the old one, a restore withdraws the best, never a second). Pure: rank
/// decides; names come from the dump (`obj_by_id` debug names, not display
/// labels).
pub fn best_banked_by_rank(
    slots: &HashMap<u8, (u16, u32)>,
    known: &HashSet<u16>,
    rank: impl Fn(&str) -> Option<usize>,
) -> Option<(u8, u16)> {
    slots
        .iter()
        .filter(|(_, (held, _))| known.contains(held))
        .min_by_key(|(_, (held, _))| {
            knowledge::obj_by_id(usize::from(*held)).and_then(|entry| entry.name).and_then(|name| rank(&name)).unwrap_or(usize::MAX)
        })
        .map(|(slot, (held, _))| (*slot, *held))
}

#[cfg(test)]
mod tests {
    use super::{best_banked_by_rank, blocked_tool, AXE_SPEC};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Restore withdraws the best banked axe, never a second or a worse
    /// one (rank order is AXE_ORDER best-first).
    #[test]
    fn bank_restore_withdraws_best_rank() {
        use std::collections::HashMap;
        let rank = |name: &str| AXE_SPEC.rank(name);
        assert!(rank("steel_axe") < rank("bronze_axe"), "steel outranks bronze");
        assert_eq!(rank("no_such_axe"), None, "unknown axes never rank");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::kit::pack::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::kit::pack::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let axes = AXE_SPEC.known_ids(&bot);
        let slots: HashMap<u8, (u16, u32)> = [(3u8, (bronze, 1u32)), (7, (steel, 1))].into_iter().collect();
        assert_eq!(best_banked_by_rank(&slots, &axes, rank), Some((7, steel)), "best banked axe wins");
        assert_eq!(best_banked_by_rank(&HashMap::new(), &axes, rank), None, "empty bank restores nothing");
    }

    /// Live 2026-11-13 (bdr53jee: shears in bank slot 48, "tool restore:
    /// bank at (3091,3245) too far from (3193,3262)", blocked x4 in 30 s
    /// while the brain's obtain closed `already_held` each time): a clean
    /// pack skips the bank trip, so the restore must know a usable tool
    /// rests in the bank to walk there itself.
    #[test]
    fn a_banked_tool_is_known_from_the_bank_view() {
        use super::{banked_tool_held, FISH_SPEC};
        use std::collections::HashMap;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert!(!banked_tool_held(&bot, &FISH_SPEC), "no bank view, nothing known");
        let main = (bot.tables.com_id)("bank_main:inv").expect("bank component");
        let net = crate::kit::pack::obj_id(&bot, "net").expect("net in tables");
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins in tables");
        bot.world.inv.insert(main, HashMap::from([(0u8, (coins, 100u32))]));
        assert!(!banked_tool_held(&bot, &FISH_SPEC), "coins are no net");
        bot.world.inv.insert(main, HashMap::from([(0u8, (coins, 100u32)), (48, (net, 1))]));
        assert!(banked_tool_held(&bot, &FISH_SPEC), "the banked net is known");
    }

    /// Missing-tool error carries the buyable name: the brain funds an
    /// obtain task from it, the session reports `stop: blocked`.
    #[test]
    fn blocked_tool_names_the_buyable() {
        assert!(matches!(blocked_tool("bronze_axe"), crate::ScriptError::Blocked { .. }));
        let crate::ScriptError::Blocked { missing } = blocked_tool("bronze_pickaxe") else {
            panic!("blocked carries missing");
        };
        assert_eq!(missing, vec!["bronze_pickaxe".to_string()]);
    }
}

#[cfg(test)]
mod keep_tests {
    use super::AXE_SPEC;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    #[test]
    fn fallback_order_skips_unaffordable_with_16() {
        use crate::kit::pack::obj_price;
        let bronze = obj_price("bronze_axe");
        let iron = obj_price("iron_axe");
        let steel = obj_price("steel_axe");
        assert_eq!((bronze, iron, steel), (16, 56, 200), "dump prices, got: {bronze}/{iron}/{steel}");
        let cands = ["steel_axe", "iron_axe", "bronze_axe"];
        let affordable: Vec<&&str> = cands.iter().filter(|name| obj_price(name) <= 16).collect();
        assert_eq!(affordable, vec![&"bronze_axe"], "16 coins fund bronze only, got: {affordable:?}");
    }

    /// Bank keep holds one axe only: the best carried. Coins always land
    /// in the bank, never ride to the spot; inferior axes are banked, not
    /// doubled up (the server chops with the best carried axe at any
    /// level, so a second axe is dead weight).
    #[test]
    fn chop_keep_never_holds_coins() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins in tables");
        let bronze = crate::kit::pack::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::kit::pack::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32)), (1, (steel, 1))].into_iter().collect());
        let keep = AXE_SPEC.keep_ids(&bot);
        assert_eq!(keep, vec![steel], "one axe in the pack: the best, got: {keep:?}");
        assert!(!keep.contains(&coins), "coins always banked, never carried");
        assert!(!keep.contains(&bronze), "inferior axe banked, never doubled");
    }
}

#[cfg(test)]
mod fish_order_tests {
    use super::{FISH_ORDER, FISH_SPEC};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    #[test]
    fn fish_order_matches_tier_levels() {
        let level_of = |tool: &str| {
            lifescape_gamedata::fishing::all_fish_tiers()
                .iter()
                .filter(|tier| tier.tool == tool)
                .map(|tier| tier.level)
                .max()
        };
        let mut prev = i64::MAX;
        for tool in FISH_ORDER {
            let level = level_of(tool).expect("every ordered tool is a tier tool");
            assert!(level < prev, "{tool} breaks the best-first order");
            prev = level;
        }
        assert_eq!(level_of("fishing_rod"), Some(25), "the rod serves two tiers, its rank is the higher one");
        assert_eq!(FISH_SPEC.rank("lobster_pot"), Some(0));
        assert_eq!(FISH_SPEC.rank("no_such_tool"), None, "unknown never ranks");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert_eq!(FISH_SPEC.best_held(&bot), None, "empty pack holds nothing");
        let rod = obj_id(&bot, "fishing_rod").expect("fishing_rod in tables");
        let fly = obj_id(&bot, "fly_fishing_rod").expect("fly_fishing_rod in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (rod, 1u32)), (1, (fly, 1))].into_iter().collect());
        assert_eq!(FISH_SPEC.best_held(&bot), Some("fishing_rod".to_string()), "rod 25 beats fly 20");
    }
}

#[cfg(test)]
mod gather_tests {
    use super::{best_banked_by_rank, reattach_tool, ToolSpec};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;


    /// Tool rank: best held wins, keep holds one tool only, unknown tools
    /// never rank. State reads only, no shop walk needed for the rank.
    #[test]
    fn tool_rank_picks_best_held() {
        const ORDER: [&str; 3] = ["steel_axe", "iron_axe", "bronze_axe"];
        const SPEC: ToolSpec = ToolSpec { order: &ORDER };
        assert_eq!(SPEC.rank("steel_axe"), Some(0));
        assert_eq!(SPEC.rank("no_such_tool"), None, "unknown never ranks");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert_eq!(SPEC.best_held(&bot), None, "empty pack holds nothing");
        let bronze = crate::kit::pack::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::kit::pack::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32)), (1, (steel, 1))].into_iter().collect());
        assert_eq!(SPEC.best_held(&bot), Some("steel_axe".to_string()), "best held wins");
        assert_eq!(SPEC.keep_ids(&bot), vec![steel], "one tool in the pack: the best");
        assert!(!SPEC.keep_ids(&bot).contains(&bronze), "inferior banks, never doubles");
        assert!(SPEC.known_ids(&bot).contains(&bronze), "known covers the banked types too");
    }


    /// Upgrade shortlist: only better-ranked, shop-gated by the caller
    /// predicate (`usable`); the cheapest price is the min of the cands.
    #[test]
    fn upgrade_lists_only_usable_better_tools() {
        const ORDER: [&str; 3] = ["steel_axe", "iron_axe", "bronze_axe"];
        const SPEC: ToolSpec = ToolSpec { order: &ORDER };
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::kit::pack::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32))].into_iter().collect());
        let (none, cheapest) = SPEC.cheapest_upgrade(&bot, |_| false);
        assert!(none.is_empty() && cheapest == 0, "nothing usable, nothing listed");
        let (cands, _) = SPEC.cheapest_upgrade(&bot, |_| true);
        assert!(!cands.contains(&"bronze_axe".to_string()), "held rank never lists itself");
        assert!(cands.iter().all(|name| SPEC.rank(name) < SPEC.rank("bronze_axe")), "only better ranks, got: {cands:?}");
    }


    /// Reattach without a handle never sends: unknown handle and empty
    /// pack both return Ok(false) before any `op_held_u` (an offline send
    /// would fail, so Ok(false) proves nothing was attempted).
    #[test]
    fn reattach_without_handle_never_sends() {
        const ORDER: [&str; 2] = ["steel_pickaxe", "bronze_pickaxe"];
        const SPEC: ToolSpec = ToolSpec { order: &ORDER };
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert!(!reattach_tool(&mut bot, "no_such_handle", "_pickaxehead", &SPEC).expect("unknown handle never sends"));
        assert!(!reattach_tool(&mut bot, "macro_pickaxehandle", "_pickaxehead", &SPEC).expect("empty pack never sends"));
    }


    /// Bank restore withdraws the best-ranked tool, never a second or a
    /// worse one (rank fn decides, dump debug names, unknown never ranks).
    #[test]
    fn banked_best_rank_prefers_steel() {
        fn rank(name: &str) -> Option<usize> {
            ["steel_pickaxe", "bronze_pickaxe"].iter().position(|tool| *tool == name)
        }
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::kit::pack::obj_id(&bot, "bronze_pickaxe").expect("bronze_pickaxe in tables");
        let steel = crate::kit::pack::obj_id(&bot, "steel_pickaxe").expect("steel_pickaxe in tables");
        let known: std::collections::HashSet<u16> = [bronze, steel].into_iter().collect();
        let slots: std::collections::HashMap<u8, (u16, u32)> =
            [(3u8, (bronze, 1u32)), (7, (steel, 1))].into_iter().collect();
        assert_eq!(best_banked_by_rank(&slots, &known, rank), Some((7, steel)), "best banked tool wins");
        assert_eq!(best_banked_by_rank(&std::collections::HashMap::new(), &known, rank), None, "empty bank restores nothing");
    }
}
