//! Miner: best F2P rock for the level, banks full loads, sells ore to fund
//! pickaxe upgrades, reattaches lost heads instead of rebuying.
//! (Gather loop, bank trip, sell, upgrade.)

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, buy_stock, check_events, dismiss_modal, free_slots, hitpoints, learn_level, nearest_bank, obj_id,
    obj_price, pack_ids, pack_total, sell_for_target, walk_checked, WorkOutcome, GONE_BAN_TICKS,
};
use crate::kit::spots::{drop_occupied, OCCUPIED_RADIUS};
use crate::skill_gather::{best_banked_by_rank, reattach_tool, work_acked, BanSet, ToolSpec, MINING_ACKS};
use crate::skill_trade::buy_best_affordable;
use crate::{ScriptError, ScriptResult};

const MINING_STAT: u8 = 14;
const NEAR_BANK_TILES: i64 = 150;
const ROUND_IDLE_TICKS: u64 = 12;
const HANDLE_NAME: &str = "macro_pickaxehandle";

/// Best pickaxe first.
pub const PICK_ORDER: [&str; 6] =
    ["rune_pickaxe", "adamant_pickaxe", "mithril_pickaxe", "steel_pickaxe", "iron_pickaxe", "bronze_pickaxe"];
/// Wield/mining level reqs (content pickaxes config; the obj dump carries
/// no params, so the six values live here).
pub const PICK_REQS: [(&str, i64); 6] =
    [("bronze_pickaxe", 0), ("iron_pickaxe", 0), ("steel_pickaxe", 6), ("mithril_pickaxe", 21), ("adamant_pickaxe", 31), ("rune_pickaxe", 41)];

fn usable_tiers() -> Vec<knowledge::rocks::RockTier> {
    knowledge::rocks::all_rock_tiers()
        .into_iter()
        .filter(|tier| !tier.product.is_empty() && tier.product_members == Some(false) && tier.product != "blankrune")
        .collect()
}

fn tier_price(tier: &knowledge::rocks::RockTier) -> i64 {
    obj_price(&tier.product)
}

fn pick_tier(tiers: &[knowledge::rocks::RockTier], level: i64) -> Option<knowledge::rocks::RockTier> {
    let mut best: Option<&knowledge::rocks::RockTier> = None;
    for tier in tiers {
        if tier.level <= level && best.as_ref().is_none_or(|known| (tier.level, tier_price(tier)) > (known.level, tier_price(known))) {
            best = Some(tier);
        }
    }
    best.cloned()
}

/// Rock spawns for the tier's loc names within range of the bank.
fn tier_spawns(tier: &knowledge::rocks::RockTier, bank: &knowledge::banks::Bank) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.rocks.iter().any(|rock| *rock == name) {
            for spawn in entry.spawns {
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

fn pick_rock_spot(
    bot: &mut Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &BanSet,
    now: u64,
) -> Option<(knowledge::rocks::RockTier, i32, i32)> {
    let mut affordable: Vec<&knowledge::rocks::RockTier> =
        tiers.iter().filter(|tier| tier.level <= level).collect();
    affordable.sort_by_key(|tier| (-tier.level, -tier_price(tier)));
    for tier in affordable {
        let mut cands: Vec<(i64, (i32, i32))> = Vec::new();
        for (spot_x, spot_z) in tier_spawns(tier, bank) {
            if banned.is_banned((spot_x, spot_z), now) {
                continue;
            }
            // shortcut: no hunter table in the gamedata dump (Npc carries no
            // hunts_players flag), so spots skip the static safety filter;
            // hurt-driven flee still relocates mid-load.
            let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
            cands.push((dist, (spot_x, spot_z)));
        }
        // Occupation perçue: dodge rocks another visible player works
        // while free ones exist; a full house keeps the pool.
        let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
        if let Some((_, (spot_x, spot_z))) = cands.into_iter().min_by_key(|(dist, _)| *dist) {
            return Some((tier.clone(), spot_x, spot_z));
        }
    }
    None
}

/// Live loc id at a tile (live stream first, static template as fallback).
/// The worker paints nothing: the fallback is one `tiles_flags` batch to
/// the nav service, which reads its snapshot. Live still wins (a depleted
/// rock streams its change), so `rock_still_there` keeps deciding below.
/// A down service stays loud (`nav-down` via `ScriptError::Sdk`); only a
/// tile the service has no template for keeps the `Usage` error.
fn live_loc_id(bot: &mut Bot, rock_x: i32, rock_z: i32) -> Result<u16, ScriptError> {
    if let Some((loc, _, _)) = bot.world.loc_at(rock_x, rock_z, bot.world.level) {
        return Ok(loc);
    }
    let level = bot.world.level;
    let rows = bot.tiles_flags(&[(rock_x, rock_z, level)]).map_err(ScriptError::Sdk)?;
    if let Some((loc, _, _)) = rows.into_iter().next().and_then(|row| row.loc) {
        return Ok(loc);
    }
    Err(ScriptError::Usage(format!("no loc at {rock_x},{rock_z} from {:?}", bot.position())))
}

/// True unless the server confirmed a depleted rock here (unknown = assume
/// standing, never assume depletion).
fn rock_still_there(bot: &mut Bot, spot_x: i32, spot_z: i32, tier: &knowledge::rocks::RockTier) -> bool {
    let Ok(loc) = live_loc_id(bot, spot_x, spot_z) else {
        return true;
    };
    match knowledge::loc_by_id(usize::from(loc)) {
        Some(entry) => {
            let debug = entry.name.unwrap_or_default().to_lowercase();
            tier.rocks.iter().any(|rock| *rock == debug)
        }
        None => true,
    }
}

/// Shared rank: best pickaxe first, level gates pass through `usable`
/// (`ToolSpec` decides, the tables carry no params). One const, reused by
/// every caller (never rebuilt per call).
const PICK_SPEC: ToolSpec = ToolSpec { order: &PICK_ORDER };

/// Rank in PICK_ORDER (0 = best).
fn pick_rank(name: &str) -> Option<usize> {
    PICK_SPEC.rank(name)
}

fn best_pickaxe(bot: &Bot) -> Option<String> {
    PICK_SPEC.best_held(bot)
}

fn known_pick_ids(bot: &Bot) -> HashSet<u16> {
    PICK_SPEC.known_ids(bot)
}

/// Reattach a handle + head carried instead of rebuying (shared pack op).
fn reattach(bot: &mut Bot) -> Result<bool, ScriptError> {
    reattach_tool(bot, HANDLE_NAME, "_pickaxehead", &PICK_SPEC)
}

fn cheapest_upgrade(bot: &Bot, level: i64) -> (Vec<String>, i64) {
    // shortcut: only surface-reachable shops stock upgrades (Nurmof sits
    // down the dwarven-mine stairs), so usually just bronze; better ones
    // land as soon as stairs traversal does.
    PICK_SPEC.cheapest_upgrade(bot, |name| PICK_REQS.iter().find(|(tool, _)| tool == &name).is_some_and(|(_, req)| level >= *req))
}

/// Click once, then only re-click after a quiet round (the server chains
/// attempts itself while in range).
fn work_spot(
    bot: &mut Bot,
    rock_x: i32,
    rock_z: i32,
    ore_ids: &[u16],
    had_ore: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable rock: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, rock_x, rock_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 1 {
        if !crate::skill_walk::step_adjacent(bot, rock_x, rock_z)? {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let rock_loc = live_loc_id(bot, rock_x, rock_z)?;
    let mut gained = 0u32;
    let mut silences = 0u32;
    let mut acked = false;
    let mut need_click = true;
    let mut seen_msgs = bot.world.messages.len();
    let hitpoints_before = hitpoints(bot);
    loop {
        if need_click {
            bot.log(&format!("click rock at ({rock_x},{rock_z})"));
            bot.intend(&format!("mining a rock at ({rock_x},{rock_z})"));
            bot.op_loc(1, rock_x, rock_z, rock_loc)?;
            seen_msgs = bot.world.messages.len();
            need_click = false;
        }
        let target = bot.world.ticks + ROUND_IDLE_TICKS;
        let mut quiet = true;
        while bot.world.ticks < target {
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
                if lowered.contains("mining level of") {
                    return Ok((WorkOutcome::LevelLocked, gained));
                }
                if lowered.contains("do not have a pickaxe") {
                    return Ok((WorkOutcome::NoTool, gained));
                }
                if lowered.contains("too full to hold any more") {
                    return Ok((WorkOutcome::Full, gained));
                }
                if lowered.contains("no ore currently available") || lowered.contains("no minerals currently available") {
                    return Ok((WorkOutcome::Gone, gained));
                }
            }
            let now_have: u32 = ore_ids.iter().map(|id| pack_total(bot, *id)).sum();
            if now_have > had_ore + gained {
                gained = now_have - had_ore;
                quiet = false;
                break;
            }
        }
        if !quiet {
            continue;
        }
        silences += 1;
        // Swing acked and the rock stage unchanged (depletion is a
        // streamed loc_change): the player IS mining — keep waiting.
        // Quiet rounds still re-click as reachability probes: only a
        // click surfaces a refusal.
        acked = acked || work_acked(bot, seen_msgs, &MINING_ACKS);
        let live = live_loc_id(bot, rock_x, rock_z).is_ok_and(|id| id == rock_loc);
        if silences >= 2 && !(acked && live) {
            return Ok((WorkOutcome::Gone, gained));
        }
        if silences == 2 && acked && live {
            bot.log(&format!("still mining at ({rock_x},{rock_z}), waiting out the dry streak"));
        }
        need_click = true;
    }
}

/// Withdraw a banked pickaxe (or its handle+head) and reattach it.
/// Cheaper than rebuying. Reopens the bank: call after a bank trip.
fn restore_from_bank(bot: &mut Bot, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_pickaxe(bot).is_some() {
        return Ok(true);
    }
    let Ok(handle) = obj_id(bot, HANDLE_NAME) else {
        return Ok(false);
    };
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with("_pickaxehead"))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    if heads.is_empty() {
        return Ok(false);
    }
    let Some((booth_x, booth_z, booth_loc)) = crate::skill_common::live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
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
    let picks = known_pick_ids(bot);
    // One pickaxe in the pack: withdraw the best banked one, never a
    // second (shared rank, same as the axe loop).
    if let Some((slot, held)) = best_banked_by_rank(&slots, &picks, pick_rank) {
        bot.inv_button(1, held, slot, main_com)?;
        bot.pump(10)?;
        bot.close_modal()?;
        bot.pump(5)?;
        if best_pickaxe(bot).is_some() {
            bot.log("ok withdrew banked pickaxe");
            return Ok(true);
        }
        return Ok(false);
    }
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
    if reattach(bot)? {
        bot.log("ok restored pickaxe from banked pieces");
        return Ok(true);
    }
    Ok(false)
}

fn recover_pickaxe(bot: &mut Bot, coins: u16, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_pickaxe(bot).is_some() {
        return Ok(true);
    }
    if reattach(bot)? {
        return Ok(true);
    }
    let bronze_price = obj_price("bronze_pickaxe") as u32;
    if pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0 {
        bot.log("ok replacement pickaxe bought");
        return Ok(true);
    }
    let Some(home) = nearest_bank(bot).ok() else {
        return Ok(false);
    };
    let keep = vec![coins];
    if bank_trip(bot, &home, &keep, bronze_price.saturating_sub(pack_total(bot, coins)))? == 0 {
        let _ = bank;
        return Ok(false);
    }
    if best_pickaxe(bot).is_none() && restore_from_bank(bot, &home)? {
        bot.log("ok replacement pickaxe restored from bank");
        return Ok(true);
    }
    if best_pickaxe(bot).is_some() {
        bot.log("ok replacement pickaxe restored");
        return Ok(true);
    }
    Ok(pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0)
}

/// Mining policy on the shared driver: full pack ends the load, a lost
/// pickaxe exits for caller-side recovery (reattach, shop, bank), tier
/// upgrades and dead rocks drop the spot.
struct MineOps<'a> {
    tiers: &'a [knowledge::rocks::RockTier],
    bank: &'a knowledge::banks::Bank,
    coins: u16,
    ore_ids: Vec<u16>,
    level: i64,
}

impl crate::skill_loop::LoopOps for MineOps<'_> {
    type Spot = knowledge::rocks::RockTier;

    fn is_full(&self, bot: &Bot) -> bool {
        free_slots(bot) <= usize::from(pack_total(bot, self.coins) > 0)
    }

    fn has_tool(&self, bot: &Bot) -> bool {
        best_pickaxe(bot).is_some()
    }

    fn gain_of(&self, bot: &Bot, _spot: &Self::Spot) -> u32 {
        self.ore_ids.iter().map(|id| pack_total(bot, *id)).sum()
    }

    fn pick(&mut self, bot: &mut Bot, state: &mut crate::skill_loop::LoopState) -> Option<(Self::Spot, i32, i32)> {
        self.level = learn_level(bot, MINING_STAT, self.level);
        let now = bot.world.ticks;
        let found = pick_rock_spot(bot, self.tiers, self.bank, self.level, &state.banned, now);
        match found {
            Some((ref tier, spot_x, spot_z)) => {
                bot.log(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
                bot.intend(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
            }
            None => {
                bot.log("no rock spot, waiting for respawns");
                bot.intend("waiting for a safe rock spot");
            }
        }
        found
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::skill_loop::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
    ) -> bool {
        self.level = learn_level(bot, MINING_STAT, self.level);
        if let Some(best) = pick_tier(self.tiers, self.level) {
            if best.level > spot.level {
                bot.log("level up, switching tier");
                return false;
            }
        }
        if !rock_still_there(bot, at.0, at.1, spot) {
            bot.log(&format!("rock gone at ({},{})", at.0, at.1));
            state.banned.ban_for(at, bot.world.ticks, GONE_BAN_TICKS);
            return false;
        }
        true
    }

    fn work(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::skill_loop::LoopState,
        _spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError> {
        work_spot(bot, at.0, at.1, &self.ore_ids, had)
    }
}

fn mine_load(
    bot: &mut Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    level: i64,
) -> Result<(bool, i64), ScriptError> {
    let ore_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut state = crate::skill_loop::LoopState::fresh(bot);
    loop {
        if best_pickaxe(bot).is_none() {
            // Recovery walks (shop, bank) are not mining: don't bill
            // them to the no-ore stall budget.
            let spent = bot.world.ticks;
            if !recover_pickaxe(bot, coins, bank)? {
                bot.log("pickaxe lost, banking load");
                return Ok((true, level));
            }
            state.idle_since += bot.world.ticks - spent;
        }
        let mut ops = MineOps { tiers, bank, coins, ore_ids: ore_ids.clone(), level };
        if crate::skill_loop::run_load(bot, &crate::skill_loop::LoadCfg::mining(), &mut state, &mut ops)? {
            return Ok((true, learn_level(bot, MINING_STAT, ops.level)));
        }
        if best_pickaxe(bot).is_none() {
            continue;
        }
        return Ok((false, learn_level(bot, MINING_STAT, ops.level)));
    }
}

/// Mine banked loads in the open session (0 = forever).
pub fn run_mining(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let loads: u32 = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0);
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = usable_tiers();
    let mut level = learn_level(bot, MINING_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut bank_coins = 0u32;
    if best_pickaxe(bot).is_none() || pack_ids(bot).iter().any(|id| !known_pick_ids(bot).contains(id) && *id != coins) {
        if reattach(bot)? {
            bot.log("ok pickaxe reattached from pack, no bootstrap needed");
        }
        let keep: Vec<u16> = known_pick_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if best_pickaxe(bot).is_none() && restore_from_bank(bot, &bank)? {
            bot.log("ok pickaxe restored from bank");
        }
        if best_pickaxe(bot).is_none() {
            let bronze_price = obj_price("bronze_pickaxe") as u32;
            let need_cross = crate::skill_common::toll_gate_x(bot)
            .is_some_and(|line| (bank.x as i32 >= line) != (bot.world.x >= line));
            let target = bronze_price + if need_cross { 10 } else { 0 };
            if pack_total(bot, coins) < target {
                bank_coins = bank_trip(bot, &bank, &keep, u32::MAX / 2)?;
            }
            if need_cross && pack_total(bot, coins) >= target && !crate::skill_common::cross_toll_gate(bot)? {
                return Err(ScriptError::Usage(format!("cannot cross toll gate at {:?}", bot.position())));
            }
            if pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0 {
                bot.log("ok bootstrapped bronze pickaxe");
            } else {
                return Err(ScriptError::Usage(format!(
                    "cannot bootstrap: no pickaxe, {} coins",
                    pack_total(bot, coins)
                )));
            }
        }
    }
    let mut done = 0u32;
    let mut toolless = 0u32;
    while loads == 0 || done < loads {
        check_events(bot)?;
        let (ok, new_level) = mine_load(bot, &tiers, &mut bank, coins, level)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Usage(format!("mine failed at {:?}", bot.position())));
        }
        let (cands, cheapest) = cheapest_upgrade(bot, level);
        if !cands.is_empty() {
            sell_for_target(bot, &ore_ids_for(bot, &tiers), coins, cheapest as u32)?;
        }
        let keep: Vec<u16> = known_pick_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, cheapest as u32)?;
        // Post-bank upgrade: only what the pack affords, best-first with
        // downgrade on shop failure (shared aborts: death/conn/interrupt).
        let _ = buy_best_affordable(bot, &cands)?;
        toolless = if best_pickaxe(bot).is_none() { toolless + 1 } else { 0 };
        if toolless >= 2 {
            return Err(ScriptError::Usage(format!("no pickaxe and cannot rebuy, aborting at {:?}", bot.position())));
        }
        done += 1;
        bot.log(&format!("ok load {done} banked (mining={level})"));
    }
    let _ = bank_coins;
    Ok(0)
}

fn ore_ids_for(bot: &Bot, tiers: &[knowledge::rocks::RockTier]) -> Vec<u16> {
    tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables_live::attach_live_tables;

    /// Mapless worker against a real nav service (repo dumps): no painted
    /// world, only the plan handle — the same shape as a live worker.
    fn served_mining_bot(socket_tag: &str) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let data = root.join("LifeScape/gamedata/data");
        let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
        let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = lifescape_nav::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = std::sync::Arc::new(lifescape_nav::tables::Tables::load(&data).expect("tables"));
        let version = format!("test-mine-{socket_tag}");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, version.clone()),
            &lifescape_nav::service::test_socket(socket_tag),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), socket_tag));
        bot.plan_data_version = Some(version);
        bot.last_keepalive = Some(std::time::Instant::now());
        (bot, service)
    }

    /// First dump rock spawn the service has a template for, with its loc:
    /// the fallback target, found in one batch.
    fn templated_rock_spawn(bot: &mut Bot) -> (i32, i32, u16) {
        let mut spawns: Vec<(i32, i32)> = Vec::new();
        for tier in usable_tiers() {
            for entry in knowledge::all_locs() {
                let name = entry.name.clone().unwrap_or_default().to_lowercase();
                if tier.rocks.iter().any(|rock| *rock == name) {
                    for spawn in entry.spawns {
                        if spawn.level == 0 {
                            spawns.push((spawn.x as i32, spawn.z as i32));
                        }
                    }
                }
            }
        }
        spawns.sort();
        spawns.dedup();
        let tiles: Vec<(i32, i32, i32)> = spawns.iter().take(64).map(|(x, z)| (*x, *z, 0)).collect();
        let rows = bot.tiles_flags(&tiles).expect("flags answer");
        spawns
            .into_iter()
            .take(64)
            .zip(rows)
            .find_map(|((x, z), row)| row.loc.map(|(loc, _, _)| (x, z, loc)))
            .expect("a templated rock spawn")
    }

    /// Unstreamed rock tile: the service template answers the loc instead
    /// of ending the run with `Usage` (same hole as the woodcut twin).
    /// Live still wins once streamed.
    #[test]
    fn live_loc_id_falls_back_to_the_service_template() {
        let (mut bot, _service) = served_mining_bot("mine-loc-fallback");
        let (rock_x, rock_z, template) = templated_rock_spawn(&mut bot);
        bot.world.locs.remove(&(rock_x, rock_z, 0));
        assert!(bot.world.loc_at(rock_x, rock_z, 0).is_none(), "nothing streamed yet");
        assert_eq!(live_loc_id(&mut bot, rock_x, rock_z).expect("static template answers"), template);
        // Live wins once streamed.
        bot.world.locs.insert((rock_x, rock_z, 0), Some((template.wrapping_add(1), 0, 0)));
        assert_eq!(
            live_loc_id(&mut bot, rock_x, rock_z).expect("live answers"),
            template.wrapping_add(1),
            "live beats the template"
        );
    }

    /// A tile the service has no template for keeps the `Usage` error
    /// (a spot with genuinely no rock), never a guessed id.
    #[test]
    fn live_loc_id_without_template_stays_usage() {
        let (mut bot, _service) = served_mining_bot("mine-loc-empty");
        // One batch over open grass: the first tile without a template
        // proves the error path.
        let cands: Vec<(i32, i32, i32)> = (0..32).map(|index| (3000 + index, 3300, 0)).collect();
        let rows = bot.tiles_flags(&cands).expect("flags answer");
        let empty = cands
            .iter()
            .zip(rows)
            .find(|(_, row)| row.loc.is_none())
            .map(|(tile, _)| *tile)
            .expect("open grass somewhere");
        let err = live_loc_id(&mut bot, empty.0, empty.1).expect_err("no template is Usage");
        assert!(matches!(err, ScriptError::Usage(_)), "Usage, got: {err:?}");
    }
}
