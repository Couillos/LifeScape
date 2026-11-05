//! Fisher: best spot for the level, banks full loads, sells fish to fund
//! tool and bait upgrades. Same shape as `scripts/fishing/run.py`.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, buy_stock, check_events, dismiss_modal, free_slots, hitpoints, learn_level, nearest_bank, obj_id,
    obj_price, pack_ids, pack_total, sell_for_target, walk_checked, wait_pack_streamed, WorkOutcome, FLEE_BAN_TICKS,
    GONE_BAN_TICKS, MAX_FLEES_PER_LOAD, STALL_TICKS,
};
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
    banned: &HashSet<(i32, i32)>,
    pinned: &knowledge::banks::Bank,
) -> Option<(knowledge::fishing::FishTier, i32, i32, knowledge::banks::Bank)> {
    let mut tiers = knowledge::fishing::all_fish_tiers();
    tiers.sort_by_key(|tier| -tier.level);
    for tier in tiers.into_iter().filter(|tier| tier.level <= level) {
        let mut cands: Vec<(i32, i32, i32)> = Vec::new();
        for npc_id in knowledge::fishing::spot_npc_ids(tier.category) {
            if let Some(entry) = knowledge::npc_by_id(npc_id) {
                for spawn in &entry.spawns {
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
            if spot_level != bot.world.level || banned.contains(&(spot_x, spot_z)) {
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

/// Best tool held, by tier level.
fn best_tool(bot: &Bot) -> Option<String> {
    let held = pack_ids(bot);
    let mut best: Option<(i64, String)> = None;
    for tier in knowledge::fishing::all_fish_tiers() {
        if let Ok(tool) = obj_id(bot, tier.tool) {
            if held.contains(&tool) && best.as_ref().is_none_or(|(level, _)| tier.level > *level) {
                best = Some((tier.level, tier.tool.to_string()));
            }
        }
    }
    best.map(|(_, tool)| tool)
}

fn known_tool_ids(bot: &Bot) -> HashSet<u16> {
    knowledge::fishing::all_fish_tiers().into_iter().filter_map(|tier| obj_id(bot, tier.tool).ok()).collect()
}

fn cheapest_upgrade(bot: &Bot, level: i64) -> (Vec<String>, i64) {
    let current_level =
        best_tool(bot).and_then(|tool| knowledge::fishing::all_fish_tiers().into_iter().find(|tier| tier.tool == tool))
            .map(|tier| tier.level)
            .unwrap_or(-1);
    let owned: Vec<knowledge::fishing::FishTier> = knowledge::fishing::all_fish_tiers()
        .into_iter()
        .filter(|tier| tier.level > current_level && tier.level <= level)
        .filter(|tier| crate::skill_common::shop_stock_keeper(bot, tier.tool).is_some())
        .collect();
    let mut names: Vec<String> = owned.iter().map(|tier| tier.tool.to_string()).collect();
    names.sort_by_key(|tool| {
        -knowledge::fishing::all_fish_tiers()
            .into_iter()
            .find(|tier| tier.tool == *tool)
            .map(|tier| tier.level)
            .unwrap_or(0)
    });
    let cheapest = names.iter().map(|tool| obj_price(tool)).min().unwrap_or(0);
    (names, cheapest)
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
    if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 4
        && !walk_checked(bot, spot_x, spot_z, 6000).unwrap_or(false)
    {
        return Ok((WorkOutcome::Unreachable, 0));
    }
    if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 2 {
        let mut neighbors: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (spot_x + dx, spot_z + dz)))
            .filter(|tile| *tile != (spot_x, spot_z))
            .collect();
        neighbors.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        let mut stepped = false;
        for (next_x, next_z) in neighbors {
            if bot.walk_to(next_x, next_z, bot.world.level, 100).is_ok() {
                stepped = true;
                break;
            }
            if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 4 {
                return Ok((WorkOutcome::Unreachable, 0));
            }
        }
        if !stepped {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let Some((slot, live_x, live_z)) = nearest_live(bot, tier.category, spot_x, spot_z) else {
        return Ok((WorkOutcome::Gone, 0));
    };
    let _ = (live_x, live_z);
    let mut gained = 0u32;
    println!("click {} at ({spot_x},{spot_z})", tier.action);
    bot.intend(&format!("fishing {}", tier.products.first().unwrap_or(&"?")));
    bot.op_npc(tier.op as u8, slot)?;
    let mut seen_msgs = bot.world.messages.len();
    let mut idle_since = bot.world.ticks;
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
        if bot.world.ticks - idle_since > 60 {
            return Ok((WorkOutcome::Gone, gained));
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

#[allow(clippy::too_many_arguments)]
fn fish_load(
    bot: &mut Bot,
    level: i64,
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    side_shop: u16,
    main_shop: u16,
) -> Result<(bool, i64), ScriptError> {
    let tiers = knowledge::fishing::all_fish_tiers();
    let mut product_ids: Vec<u16> = Vec::new();
    for tier in &tiers {
        for product in tier.products {
            if let Ok(id) = obj_id(bot, product) {
                product_ids.push(id);
            }
        }
    }
    let mut bait_ids: HashMap<&str, u16> = HashMap::new();
    for tier in &tiers {
        if let Some(bait) = tier.bait {
            if let Ok(id) = obj_id(bot, bait) {
                bait_ids.insert(bait, id);
            }
        }
    }
    let mut banned: HashMap<(i32, i32), u64> = HashMap::new();
    let mut bait_ban_until = 0u64;
    let idle_since = bot.world.ticks;
    let mut last_hitpoints = hitpoints(bot);
    let mut flees = 0u32;
    let mut walk_hurts = 0u32;
    let mut spot: Option<(knowledge::fishing::FishTier, i32, i32)> = None;
    loop {
        check_events(bot)?;
        if free_slots(bot) <= usize::from(pack_total(bot, coins) > 0) {
            return Ok((true, level));
        }
        let level = learn_level(bot, FISH_STAT, level);
        let best = knowledge::fishing::best_fish_tier(level);
        if let (Some(picked), Some(top)) = (&spot, best) {
            if picked.0.level < top.level {
                println!("level up, switching tier");
                spot = None;
            }
        }
        if hitpoints(bot) == 0 {
            println!("died, respawning");
            bot.intend("died, respawning");
            let target = bot.world.ticks + 40;
            while bot.world.ticks < target && hitpoints(bot) == 0 {
                bot.pump(5)?;
            }
            last_hitpoints = hitpoints(bot);
            spot = None;
            continue;
        }
        if hitpoints(bot) < last_hitpoints {
            flees += 1;
            println!("hurt, fleeing");
            bot.intend("fleeing a hunter");
            if let Some((_, spot_x, spot_z)) = &spot {
                banned.insert((*spot_x, *spot_z), bot.world.ticks + FLEE_BAN_TICKS);
            }
            if flees > MAX_FLEES_PER_LOAD {
                println!("too many attacks, aborting load");
                return Ok((false, level));
            }
            wait_ticks(bot, 200)?;
            spot = None;
            last_hitpoints = hitpoints(bot);
            continue;
        }
        last_hitpoints = hitpoints(bot);
        let now = bot.world.ticks;
        banned.retain(|_, until| *until > now);
        if spot.is_none() {
            let usable: Vec<&knowledge::fishing::FishTier> = tiers
                .iter()
                .filter(|tier| {
                    tier.bait.is_none()
                        || now >= bait_ban_until
                        || tier.bait.is_some_and(|bait| bait_ids.get(bait).is_some_and(|id| pack_total(bot, *id) > 0))
                })
                .collect();
            let _ = usable;
            let mut picked = None;
            for _ in 0..16 {
                if let Some((tier, spot_x, spot_z, use_bank)) =
                    pick_fish_spot(bot, level, &banned.keys().copied().collect(), bank)
                {
                    picked = Some((tier, spot_x, spot_z, use_bank));
                    break;
                }
                println!("no fish spot, waiting for respawns");
                bot.intend("waiting for a safe fishing spot");
                wait_ticks(bot, 15)?;
            }
            let Some((tier, spot_x, spot_z, use_bank)) = picked else {
                println!("no fish spot left");
                return Ok((false, level));
            };
            *bank = use_bank;
            if !ensure_gear(bot, &tier, side_shop, main_shop, coins, bank, &mut bait_ban_until)? {
                if pack_total(bot, obj_id(bot, tier.tool).unwrap_or(0)) == 0 {
                    println!("no gear for {}", tier.tool);
                    return Ok((false, level));
                }
                spot = None;
                continue;
            }
            println!("fishing {} at ({spot_x},{spot_z})", tier.products.first().unwrap_or(&"?"));
            bot.intend(&format!("walking to the fishing spot at ({spot_x},{spot_z})"));
            match walk_checked(bot, spot_x, spot_z, 15000) {
                Ok(true) => {}
                Ok(false) => {
                    wait_ticks(bot, 200)?;
                    walk_hurts += 1;
                    if walk_hurts >= 3 {
                        banned.insert((spot_x, spot_z), bot.world.ticks + 500);
                        walk_hurts = 0;
                    }
                    continue;
                }
                Err(_) => {
                    banned.insert((spot_x, spot_z), bot.world.ticks + GONE_BAN_TICKS);
                    continue;
                }
            }
            spot = Some((tier, spot_x, spot_z));
        }
        let (tier, spot_x, spot_z) = spot.clone().expect("spot picked");
        let wanted: Vec<u16> = tier
            .products
            .iter()
            .filter_map(|product| obj_id(bot, product).ok())
            .collect();
        let had_fish: u32 = wanted.iter().map(|id| pack_total(bot, *id)).sum();
        let (result, gained) = work_spot(bot, &tier, spot_x, spot_z, &wanted, had_fish)?;
        let _ = gained;
        match result {
            WorkOutcome::Full => return Ok((true, level)),
            WorkOutcome::Moved | WorkOutcome::Gone | WorkOutcome::Threat => {
                println!("spot quiet, next one");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::NoTool | WorkOutcome::Unreachable | WorkOutcome::LevelLocked => {
                banned.insert((spot_x, spot_z), now + 500);
                spot = None;
            }
            WorkOutcome::NoBait => {
                if restock_bait(bot, &tier, coins, bank)? {
                    println!("bait restocked, resuming");
                    continue;
                }
                println!("bait out, dropping to net tier");
                bait_ban_until = bot.world.ticks + 1000;
                spot = None;
                if free_slots(bot) == 0 {
                    return Ok((true, level));
                }
            }
            WorkOutcome::Blocked => {
                println!("unknown dialog blocking fish, aborting load");
                return Ok((false, level));
            }
        }
        if bot.world.ticks - idle_since > STALL_TICKS {
            println!("stall: no fish for {STALL_TICKS} ticks");
            return Ok((false, level));
        }
    }
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
    println!("bank at ({},{})", bank.x, bank.z);
    let mut bank_coins = 0u32;
    if best_tool(bot).is_none() {
        let keep = vec![coins];
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if best_tool(bot).is_none() {
            let net_price = obj_price("net") as u32;
            if pack_total(bot, coins) >= net_price && buy_stock(bot, "net", 1)? > 0 {
                println!("ok bootstrapped net");
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
        for cand in cands {
            if buy_stock(bot, &cand, 1)? > 0 {
                println!("ok upgraded to {cand}");
                break;
            }
        }
        done += 1;
        println!("ok load {done} banked (fishing={level})");
    }
    Ok(0)
}
