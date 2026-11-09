//! Fisher: best spot for the level, banks full loads, sells fish to fund
//! tool and bait upgrades (gather loop, bank trip, sell, upgrade).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, buy_stock, check_events, dismiss_modal, free_slots, hitpoints, learn_level, nearest_bank, obj_id,
    obj_price, pack_total, sell_for_target, walk_checked, wait_pack_streamed, WorkOutcome,
};
use crate::skill_gather::{work_acked, BanSet, ToolSpec, FISHING_ACKS};
use crate::skill_trade::buy_best_affordable;
use crate::{ScriptError, ScriptResult};

const FISH_STAT: u8 = 10;
const LIVE_RADIUS: i32 = 40;
const BANK_TREK_LIMIT: i64 = 150;
const BAIT_MIN: u32 = 20;
const BAIT_WANT: u32 = 200;

/// Live NPC id whose debugname ends with the tier category tail.
fn tier_npc_matches(bot: &Bot, npc_type: u16, category: &str) -> bool {
    let tail = format!("_{category}");
    (bot.tables.npc_name)(npc_type).unwrap_or_default().ends_with(&tail)
}

/// Nearest live spot NPC for a tier, relative to a reference tile.
fn nearest_live(bot: &Bot, category: &str, near_x: i32, near_z: i32) -> Option<(u16, i32, i32)> {
    let mut best: Option<(i64, (u16, i32, i32))> = None;
    for (slot, npc) in &bot.world.npcs {
        if !tier_npc_matches(bot, npc.npc_type, category) {
            continue;
        }
        if (npc.x - bot.world.x).abs() + (npc.z - bot.world.z).abs() > LIVE_RADIUS {
            continue;
        }
        let dist = i64::from((npc.x - near_x).abs() + (npc.z - near_z).abs());
        if best.as_ref().is_none_or(|(known, _)| dist < *known) {
            best = Some((dist, (*slot, npc.x, npc.z)));
        }
    }
    best.map(|(_, found)| found)
}

/// Best affordable tier with a live or static spot near a pinned bank.
fn pick_fish_spot(
    bot: &Bot,
    level: i64,
    banned: &BanSet,
    now: u64,
    pinned: &knowledge::banks::Bank,
) -> Option<(knowledge::fishing::FishTier, i32, i32, knowledge::banks::Bank)> {
    let mut tiers = knowledge::fishing::all_fish_tiers();
    tiers.sort_by_key(|tier| -tier.level);
    for tier in tiers.into_iter().filter(|tier| tier.level <= level) {
        let mut cands: Vec<(i32, i32, i32)> = Vec::new();
        for npc_id in knowledge::fishing::spot_npc_ids(tier.category) {
            if let Some(entry) = knowledge::npc_by_id(npc_id) {
                for spawn in entry.spawns {
                    cands.push((spawn.level as i32, spawn.x as i32, spawn.z as i32));
                }
            }
        }
        for (_, npc) in &bot.world.npcs {
            if tier_npc_matches(bot, npc.npc_type, tier.category) {
                cands.push((npc.level, npc.x, npc.z));
            }
        }
        let mut best: Option<(i64, (i32, i32))> = None;
        for (spot_level, spot_x, spot_z) in cands {
            if spot_level != bot.world.level || banned.is_banned((spot_x, spot_z), now) {
                continue;
            }
            let bank = match nearest_bank_at(spot_x, spot_z, spot_level) {
                Some(bank) => bank,
                None => continue,
            };
            if (i64::from(bank.x) - pinned.x).abs() + (i64::from(bank.z) - pinned.z).abs() > 12 {
                continue;
            }
            let dist = (i64::from(spot_x) - bank.x).abs() + (i64::from(spot_z) - bank.z).abs();
            if dist > BANK_TREK_LIMIT {
                continue;
            }
            if (spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs() > 400 {
                continue;
            }
            // shortcut: no hunter table in the gamedata dump (Npc carries no
            // hunts_players flag), so spots skip the static safety filter;
            // hurt-driven flee still relocates mid-load.
            if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                best = Some((dist, (spot_x, spot_z)));
            }
        }
        if let Some((_, (spot_x, spot_z))) = best {
            let bank = nearest_bank_at(spot_x, spot_z, bot.world.level).expect("bank just found");
            return Some((tier, spot_x, spot_z, bank));
        }
    }
    None
}

fn nearest_bank_at(x: i32, z: i32, level: i32) -> Option<knowledge::banks::Bank> {
    knowledge::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level == i64::from(level))
        .min_by_key(|bank| (bank.x - i64::from(x)).abs() + (bank.z - i64::from(z)).abs())
}

/// Best tool first, by max tier level: lobster 40, harpoon 35, rod 25,
/// fly 20, net 1 (the rod serves two tiers, its rank is the higher one).
/// Matches the old best-by-level pick on every held subset.
pub const FISH_ORDER: [&str; 5] =
    ["lobster_pot", "harpoon", "fishing_rod", "fly_fishing_rod", "net"];

/// Shared rank: best held tool wins, level gates pass through `usable`.
/// One const, reused by every caller (never rebuilt per call).
const FISH_SPEC: ToolSpec = ToolSpec { order: &FISH_ORDER };

/// Best tool held, best-first.
fn best_tool(bot: &Bot) -> Option<String> {
    FISH_SPEC.best_held(bot)
}

fn known_tool_ids(bot: &Bot) -> HashSet<u16> {
    FISH_SPEC.known_ids(bot)
}

fn cheapest_upgrade(bot: &Bot, level: i64) -> (Vec<String>, i64) {
    FISH_SPEC.cheapest_upgrade(bot, |name| {
        knowledge::fishing::all_fish_tiers().iter().any(|tier| tier.tool == name && tier.level <= level)
    })
}

/// One click, then watch the chain: re-clicking restarts it, so never spam.
fn work_spot(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    spot_x: i32,
    spot_z: i32,
    product_ids: &[u16],
    had_fish: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable spot: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, spot_x, spot_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 2 {
        if !crate::skill_walk::step_adjacent(bot, spot_x, spot_z)? {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let Some((slot, live_x, live_z)) = nearest_live(bot, tier.category, spot_x, spot_z) else {
        return Ok((WorkOutcome::Gone, 0));
    };
    let _ = (live_x, live_z);
    let mut gained = 0u32;
    bot.log(&format!("click {} at ({spot_x},{spot_z})", tier.action));
    bot.intend(&format!("fishing {}", tier.products.first().unwrap_or(&"?")));
    bot.op_npc(tier.op as u8, slot)?;
    let mut seen_msgs = bot.world.messages.len();
    let mut idle_since = bot.world.ticks;
    let mut acked = false;
    let mut patient = false;
    let mut last_probe = bot.world.ticks;
    let hitpoints_before = hitpoints(bot);
    loop {
        bot.pump(2)?;
        if hitpoints(bot) < hitpoints_before {
            return Ok((WorkOutcome::Threat, gained));
        }
        if bot.world.modal.is_some() && !dismiss_modal(bot)? {
            bot.pump(10)?;
            if bot.world.modal.is_some() && !dismiss_modal(bot)? {
                return Ok((WorkOutcome::Blocked, gained));
            }
        }
        for message in bot.world.messages.get(seen_msgs..).unwrap_or(&[]) {
            let lowered = message.to_lowercase();
            if lowered.contains("can't reach that") {
                return Ok((WorkOutcome::Unreachable, gained));
            }
            if lowered.contains("need at least") && lowered.contains("fishing") {
                return Ok((WorkOutcome::LevelLocked, gained));
            }
            if lowered.contains("need a ") && (lowered.contains("to catch") || lowered.contains("to bait") || lowered.contains("to lure")) {
                return Ok((WorkOutcome::NoTool, gained));
            }
            if lowered.contains("don't have any") && lowered.contains("left") {
                return Ok((WorkOutcome::NoBait, gained));
            }
            if lowered.contains("can't carry any more") {
                return Ok((WorkOutcome::Full, gained));
            }
        }
        // Cast/attempt acked on a still-live spot: the player IS fishing
        // (dry streak), not facing a dead spot — same detector as
        // woodcut/mining (own anim never streams).
        acked = acked || work_acked(bot, seen_msgs, &FISHING_ACKS);
        seen_msgs = bot.world.messages.len();
        let slot_now = nearest_live(bot, tier.category, spot_x, spot_z);
        let Some((_, live_now_x, live_now_z)) = slot_now else {
            return Ok((WorkOutcome::Gone, gained));
        };
        if (live_now_x - spot_x).abs() + (live_now_z - spot_z).abs() > 6 {
            return Ok((WorkOutcome::Moved, gained));
        }
        let now_have: u32 = product_ids.iter().map(|id| pack_total(bot, *id)).sum();
        if now_have > had_fish + gained {
            gained = now_have - had_fish;
            idle_since = bot.world.ticks;
        }
        // Cast/attempt acked on a still-live spot: the player IS fishing
        // (dry streak), not facing a dead spot — wait, no time cap; the
        // load-level stall stays the ultimate backstop. Re-click as a
        // reachability probe every ~30 quiet ticks: only a click surfaces
        // a refusal. Unacked quiet still gives up fast: the click itself
        // never landed.
        if bot.world.ticks - idle_since > 60 && !acked {
            return Ok((WorkOutcome::Gone, gained));
        }
        if acked && bot.world.ticks - last_probe >= 30 {
            bot.op_npc(tier.op as u8, slot)?;
            last_probe = bot.world.ticks;
            seen_msgs = bot.world.messages.len();
        }
        if bot.world.ticks - idle_since > 60 && acked && !patient {
            patient = true;
            bot.log("still fishing on a live spot, waiting out the dry streak");
        }
    }
}

fn ensure_gear(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    side_shop: u16,
    main_shop: u16,
    coins: u16,
    bank: &knowledge::banks::Bank,
    bait_ban_until: &mut u64,
) -> Result<bool, ScriptError> {
    let tool = obj_id(bot, tier.tool)?;
    if pack_total(bot, tool) == 0 {
        let keep: Vec<u16> = {
            let mut keep = vec![coins];
            keep.extend(known_tool_ids(bot));
            keep
        };
        let side_bank = (bot.tables.com_id)("bank_side:inv").ok_or_else(|| ScriptError::Usage("bank com missing".to_string()))?;
        let main_bank = (bot.tables.com_id)("bank_main:inv").ok_or_else(|| ScriptError::Usage("bank com missing".to_string()))?;
        if bank_trip(bot, bank, &keep, 0)? == 0 && pack_total(bot, tool) == 0 {
            let price = obj_price(tier.tool) as u32;
            if pack_total(bot, coins) < price {
                sell_for_target(bot, &[], coins, price - pack_total(bot, coins))?;
            }
            if buy_stock(bot, tier.tool, 1)? == 0 {
                return Ok(false);
            }
        }
        let _ = (side_shop, main_shop, side_bank, main_bank);
    }
    if let Some(bait) = tier.bait {
        let bait_id = obj_id(bot, bait)?;
        if pack_total(bot, bait_id) < BAIT_MIN && !restock_bait(bot, tier, coins, bank)? {
            *bait_ban_until = bot.world.ticks + 1000;
            return Ok(false);
        }
    }
    Ok(pack_total(bot, tool) > 0)
}

fn restock_bait(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    coins: u16,
    bank: &knowledge::banks::Bank,
) -> Result<bool, ScriptError> {
    let Some(bait) = tier.bait else {
        return Ok(true);
    };
    let bait_id = obj_id(bot, bait)?;
    let need = BAIT_WANT.saturating_sub(pack_total(bot, bait_id));
    if need == 0 {
        return Ok(true);
    }
    let price = obj_price(bait).max(1) as u32;
    let shortfall = need * price - pack_total(bot, coins).min(need * price);
    if shortfall > 0 {
        sell_for_target(bot, &[], coins, shortfall)?;
    }
    if pack_total(bot, coins) < price {
        let keep: Vec<u16> = known_tool_ids(bot).into_iter().collect();
        bank_trip(bot, bank, &keep, (need * price).min(5000))?;
    }
    let affordable = pack_total(bot, coins) / price;
    buy_stock(bot, bait, need.min(affordable))?;
    Ok(pack_total(bot, bait_id) >= BAIT_MIN)
}

/// Fishing policy on the shared driver: the pack-full gate ends the
/// load, gear (tool + bait) is ensured per spot inside `work` (bank and
/// shop walks bill like the old pick phase), the trek retries in `work`,
/// bait restocks resume the same spot through `on_no_bait`.
struct FishOps<'a> {
    bank: &'a mut knowledge::banks::Bank,
    coins: u16,
    side_shop: u16,
    main_shop: u16,
    bait_ban: u64,
    level: i64,
}

impl crate::skill_loop::LoopOps for FishOps<'_> {
    type Spot = knowledge::fishing::FishTier;

    fn is_full(&self, bot: &Bot) -> bool {
        free_slots(bot) <= usize::from(pack_total(bot, self.coins) > 0)
    }

    /// Always geared at the gate: the tool check runs per spot in `work`
    /// (a broke pack aborts through `Blocked`, never here).
    fn has_tool(&self, _bot: &Bot) -> bool {
        true
    }

    fn gain_of(&self, bot: &Bot, spot: &Self::Spot) -> u32 {
        spot.products.iter().filter_map(|product| obj_id(bot, product).ok()).map(|id| pack_total(bot, id)).sum()
    }

    fn pick(&mut self, bot: &mut Bot, state: &mut crate::skill_loop::LoopState) -> Option<(Self::Spot, i32, i32)> {
        self.level = learn_level(bot, FISH_STAT, self.level);
        let now = bot.world.ticks;
        match pick_fish_spot(bot, self.level, &state.banned, now, &*self.bank) {
            Some((tier, spot_x, spot_z, use_bank)) => {
                *self.bank = use_bank;
                bot.log(&format!("fishing {} at ({spot_x},{spot_z})", tier.products.first().unwrap_or(&"?")));
                bot.intend(&format!("walking to the fishing spot at ({spot_x},{spot_z})"));
                Some((tier, spot_x, spot_z))
            }
            None => {
                bot.log("no fish spot, waiting for respawns");
                bot.intend("waiting for a safe fishing spot");
                None
            }
        }
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::skill_loop::LoopState,
        spot: &Self::Spot,
        _at: (i32, i32),
    ) -> bool {
        self.level = learn_level(bot, FISH_STAT, self.level);
        if let Some(top) = knowledge::fishing::best_fish_tier(self.level) {
            if spot.level < top.level {
                bot.log("level up, switching tier");
                return false;
            }
        }
        true
    }

    /// A lost tool condemns the spot like any other refusal (the old loop
    /// banned 500 here, never a clean drop).
    fn on_no_tool(&self) -> fn(&mut crate::skill_gather::BanSet, (i32, i32), u64, &crate::skill_loop::LoadCfg) {
        crate::skill_loop::ban_refusal
    }

    /// The old loop waited flat after a flee, never heal-gated.
    fn on_hurt(
        &mut self,
        bot: &mut Bot,
        cfg: &crate::skill_loop::LoadCfg,
        state: &mut crate::skill_loop::LoopState,
    ) -> Result<bool, ScriptError> {
        bot.log("hurt, fleeing");
        bot.intend("fleeing a hunter");
        let abort = crate::skill_loop::register_hurt(cfg, state, bot.world.ticks);
        wait_ticks(bot, 200)?;
        Ok(abort)
    }

    /// Bait ran out mid-click: restock walks bill outside the no-fish
    /// stall budget, exactly like the old `NoBait` arm.
    fn on_no_bait(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::skill_loop::LoopState,
        spot: &Self::Spot,
        _at: (i32, i32),
    ) -> Result<bool, ScriptError> {
        let spent = bot.world.ticks;
        let restocked = restock_bait(bot, spot, self.coins, self.bank)?;
        state.idle_since += bot.world.ticks - spent;
        if restocked {
            bot.log("bait restocked, resuming");
            return Ok(true);
        }
        bot.log("bait out, dropping to net tier");
        self.bait_ban = bot.world.ticks + 1000;
        Ok(false)
    }

    fn work(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::skill_loop::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError> {
        let (spot_x, spot_z) = at;
        if !ensure_gear(bot, spot, self.side_shop, self.main_shop, self.coins, self.bank, &mut self.bait_ban)? {
            if pack_total(bot, obj_id(bot, spot.tool).unwrap_or(0)) == 0 {
                bot.log(&format!("no gear for {}", spot.tool));
                // Broke without a tool: the load aborts, like the old loop.
                return Ok((WorkOutcome::Blocked, 0));
            }
            return Ok((WorkOutcome::NoBait, 0));
        }
        // Trek to the spot: three tries, then the refusal ban. The old
        // loop repicked between tries; retrying the same spot skips the
        // repick without changing the ban or the waits.
        let mut arrived = false;
        for _ in 0..3 {
            match walk_checked(bot, spot_x, spot_z, 15000) {
                Ok(true) => {
                    arrived = true;
                    break;
                }
                Ok(false) => wait_ticks(bot, 200)?,
                Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
                Err(_) => return Ok((WorkOutcome::Gone, 0)),
            }
        }
        if !arrived {
            return Ok((WorkOutcome::Unreachable, 0));
        }
        let wanted: Vec<u16> = spot.products.iter().filter_map(|product| obj_id(bot, product).ok()).collect();
        work_spot(bot, spot, spot_x, spot_z, &wanted, had)
    }
}

#[allow(clippy::too_many_arguments)]
fn fish_load(
    bot: &mut Bot,
    level: i64,
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    side_shop: u16,
    main_shop: u16,
) -> Result<(bool, i64), ScriptError> {
    let mut ops = FishOps { bank, coins, side_shop, main_shop, bait_ban: 0, level };
    let ok = crate::skill_loop::run_load(
        bot,
        &crate::skill_loop::LoadCfg::mining(),
        &mut crate::skill_loop::LoopState::fresh(bot),
        &mut ops,
    )?;
    Ok((ok, learn_level(bot, FISH_STAT, ops.level)))
}

/// Fish banked loads in the open session (0 = forever).
pub fn run_fishing(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let loads: u32 = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0);
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let side_bank = (bot.tables.com_id)("bank_side:inv")
        .ok_or_else(|| ScriptError::Usage("bank_side:inv missing from component table".to_string()))?;
    let main_bank = (bot.tables.com_id)("bank_main:inv")
        .ok_or_else(|| ScriptError::Usage("bank_main:inv missing from component table".to_string()))?;
    let side_shop = (bot.tables.com_id)("shop_template_side:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template_side:inv missing from component table".to_string()))?;
    let main_shop = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    let _ = (side_bank, main_bank);
    let coins = obj_id(bot, "coins")?;
    let mut level = learn_level(bot, FISH_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut bank_coins = 0u32;
    if best_tool(bot).is_none() {
        let keep = vec![coins];
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if best_tool(bot).is_none() {
            let net_price = obj_price("net") as u32;
            if pack_total(bot, coins) >= net_price && buy_stock(bot, "net", 1)? > 0 {
                bot.log("ok bootstrapped net");
            } else {
                return Err(ScriptError::Usage(format!(
                    "cannot bootstrap: no tool, {} coins",
                    pack_total(bot, coins)
                )));
            }
        }
    }
    let mut done = 0u32;
    while loads == 0 || done < loads {
        check_events(bot)?;
        let (ok, new_level) = fish_load(bot, level, &mut bank, coins, side_shop, main_shop)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Usage(format!("fish failed at {:?}", bot.position())));
        }
        let (cands, cheapest) = cheapest_upgrade(bot, level);
        if !cands.is_empty() {
            sell_for_target(bot, &[], coins, cheapest.max(0) as u32 - bank_coins.min(cheapest.max(0) as u32))?;
        }
        let keep: Vec<u16> = known_tool_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        // Post-bank upgrade: only what the pack affords, best-first with
        // downgrade on shop failure (shared aborts: death/conn/interrupt).
        let _ = buy_best_affordable(bot, &cands)?;
        done += 1;
        bot.log(&format!("ok load {done} banked (fishing={level})"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{best_tool, FISH_ORDER, FISH_SPEC};
    use crate::skill_common::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Static order matches the tier levels (lobster 40 > harpoon 35 >
    /// rod 25 > fly 20 > net 1), so best-held equals the old best-by-level
    /// pick on every held subset.
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
        assert_eq!(best_tool(&bot), None, "empty pack holds nothing");
        let rod = obj_id(&bot, "fishing_rod").expect("fishing_rod in tables");
        let fly = obj_id(&bot, "fly_fishing_rod").expect("fly_fishing_rod in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (rod, 1u32)), (1, (fly, 1))].into_iter().collect());
        assert_eq!(best_tool(&bot), Some("fishing_rod".to_string()), "rod 25 beats fly 20");
    }
}
