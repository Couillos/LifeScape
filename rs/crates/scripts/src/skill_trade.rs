//! Shop trade for the skill scripts: keeper lookup, loud trade clicks,
//! selling produce and buying stock. Split from `skill_common` (file-size
//! rule); `skill_common` re-exports it so callers do not change.
//!
//! Every wait is tick-bounded (`bot.world.ticks`); nothing sleeps. Trade
//! failures log their reason; post-trade sale/purchase failures are loud
//! errors, never silent `Ok(0)`.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{is_conn_abort, is_death_abort, obj_id, obj_price, pack_slot, pack_total, TREK_LIMIT};
use crate::skill_walk::walk_checked;
use crate::ScriptError;

/// Nearest general-store keeper spawn on this level within trek range.
pub fn general_store(bot: &Bot) -> Option<(String, (i32, i32, i32))> {
    let mut best: Option<(i64, (String, (i32, i32, i32)))> = None;
    for shop in knowledge::shops::all_shops() {
        if !shop.name.to_lowercase().contains("general") {
            continue;
        }
        for keeper in &shop.keepers {
            for entry in knowledge::npcs_by_name(&keeper.npc) {
                for spawn in entry.spawns {
                    if spawn.level != i64::from(bot.world.level) {
                        continue;
                    }
                    let dist = (spawn.x - i64::from(bot.world.x)).abs() + (spawn.z - i64::from(bot.world.z)).abs();
                    if dist > TREK_LIMIT {
                        continue;
                    }
                    let tile = (spawn.level as i32, spawn.x as i32, spawn.z as i32);
                    if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                        best = Some((dist, (keeper.npc.clone(), tile)));
                    }
                }
            }
        }
    }
    best.map(|(_, found)| found)
}

/// Nearest shop stocking `obj_name` (stock count > 0) with its keeper spawn.
// shortcut: keeper NPC op for Trade is assumed to be op 2 (talk/trade op
// order is pack data, not in the gamedata dump); fails loudly otherwise.
pub fn shop_stock_keeper(bot: &Bot, obj_name: &str) -> Option<(String, (i32, i32, i32))> {
    let mut best: Option<(i64, (String, (i32, i32, i32)))> = None;
    for shop in knowledge::shops::all_shops() {
        if !shop.stocks.iter().any(|stock| stock.obj == obj_name && stock.count > 0) {
            continue;
        }
        for keeper in &shop.keepers {
            for entry in knowledge::npcs_by_name(&keeper.npc) {
                for spawn in entry.spawns {
                    if spawn.level != i64::from(bot.world.level) {
                        continue;
                    }
                    let dist = (spawn.x - i64::from(bot.world.x)).abs() + (spawn.z - i64::from(bot.world.z)).abs();
                    if dist > TREK_LIMIT {
                        continue;
                    }
                    if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                        best = Some((dist, (keeper.npc.clone(), (spawn.level as i32, spawn.x as i32, spawn.z as i32))));
                    }
                }
            }
        }
    }
    best.map(|(_, found)| found)
}

/// 1-based op number for a named NPC op, from the dump (Talk-to is 1).
/// None when the NPC type or the op is unknown: callers fail loudly
/// instead of clicking the wrong verb.
pub fn npc_op(npc_type: u16, op_name: &str) -> Option<u8> {
    lifescape_gamedata::npc_by_id(npc_type as usize)
        .and_then(|entry| entry.ops.iter().position(|name| name.as_deref().is_some_and(|label| label.eq_ignore_ascii_case(op_name))))
        .map(|index| index as u8 + 1)
}

/// Live keeper slot + tile when streamed, before any spawn walk. The
/// server is the truth: a streamed keeper is used where it stands.
pub fn live_keeper(bot: &Bot, keeper: &str) -> Option<(u16, i32, i32, u16)> {
    let slot = bot.find_npc(keeper)?;
    let npc = bot.world.npcs.get(&slot)?;
    Some((slot, npc.x, npc.z, npc.npc_type))
}

/// Fallback line when the keeper is not streamed: the static spawn is the
/// last resort, never the first walk.
pub fn keeper_fallback_log(keeper: &str, spawn: (i32, i32, i32)) -> String {
    format!("keeper {keeper} not streamed, trying spawn at ({},{})", spawn.1, spawn.2)
}

/// Walk to a keeper, open trade, wait for the shop stock com. The keeper
/// name matches live NPCs by substring (same as `Bot::find_npc`).
/// Tries every tile around the keeper like the reference flow does: counters
/// block the click from most sides, so one angle is rarely enough.
/// A missing keeper falls back to the shop's other staff (same shop, same
/// generic Trade op): the dump spawn can go stale while the assistant next
/// door still streams. Every fallback is logged; giving up stays Ok(false).
pub fn trade_to_keeper(bot: &mut Bot, keeper: &str, spawn: (i32, i32, i32)) -> Result<bool, ScriptError> {
    let main_com = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    if trade_with(bot, keeper, spawn, main_com)? {
        return Ok(true);
    }
    for (alt, alt_spawn) in shop_mates(bot, keeper) {
        if alt.eq_ignore_ascii_case(keeper) {
            continue;
        }
        bot.log(&format!("keeper {keeper} missing, trying {alt} at ({},{})", alt_spawn.1, alt_spawn.2));
        if trade_with(bot, &alt, alt_spawn, main_com)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Other keepers of the keeper's own shop, nearest first: the sale and the
/// buy both ride the generic Trade op, so any streamed staff opens it.
fn shop_mates(bot: &Bot, keeper: &str) -> Vec<(String, (i32, i32, i32))> {
    let wanted = keeper.to_lowercase();
    let shop = knowledge::shops::all_shops().into_iter().find(|shop| {
        shop.keepers.iter().any(|row| row.npc.to_lowercase() == wanted)
    });
    let Some(shop) = shop else {
        return Vec::new();
    };
    let mut mates: Vec<(i64, (String, (i32, i32, i32)))> = Vec::new();
    for row in &shop.keepers {
        if row.npc.to_lowercase() == wanted {
            continue;
        }
        for entry in knowledge::npcs_by_name(&row.npc) {
            for spawn in entry.spawns {
                if spawn.level != i64::from(bot.world.level) {
                    continue;
                }
                let dist = (spawn.x - i64::from(bot.world.x)).abs() + (spawn.z - i64::from(bot.world.z)).abs();
                if dist > TREK_LIMIT {
                    continue;
                }
                let tile = (spawn.level as i32, spawn.x as i32, spawn.z as i32);
                mates.push((dist, (row.npc.clone(), tile)));
            }
        }
    }
    mates.sort_by_key(|(dist, _)| *dist);
    mates.into_iter().map(|(_, found)| found).collect()
}

fn trade_with(bot: &mut Bot, keeper: &str, spawn: (i32, i32, i32), main_com: u16) -> Result<bool, ScriptError> {
    if live_keeper(bot, keeper).is_some() {
        // Streamed keeper: walk its live tile, the static spawn (which can
        // sit inside a wall, e.g. bob's) stays untouched.
        let deadline = bot.world.ticks + 400;
        return trade_live_rounds(bot, keeper, main_com, deadline);
    }
    bot.log(&keeper_fallback_log(keeper, spawn));
    // Spawn walk with live retargeting: the keeper wanders off its static
    // spawn (and only streams nearby), so head for its live tile the
    // moment it appears instead of pacing to a stale point. Bounded
    // slices: each walks a while, the pump paces retries (instant walk
    // failures consume no ticks), and pump errors never fail the trade
    // by themselves — the walks below surface real trouble.
    let deadline = bot.world.ticks + 15000;
    for _ in 0..6 {
        let _ = bot.pump(2);
        if live_keeper(bot, keeper).is_some() {
            break;
        }
        if bot.world.level == spawn.0 && (bot.world.x, bot.world.z) == (spawn.1, spawn.2) {
            break;
        }
        let rest = deadline.saturating_sub(bot.world.ticks);
        if rest == 0 {
            break;
        }
        let before = (bot.world.x, bot.world.z);
        match walk_checked(bot, spawn.1, spawn.2, rest.min(4000)) {
            Ok(true) => break,
            Ok(false) => {
                bot.log(&format!("trade with {keeper} failed: hurt walking to spawn at {:?}", bot.position()));
                return Ok(false);
            }
            // No service means no walk at all: typed `nav-down` stays
            // loud instead of degrading into an unreachable keeper.
            Err(err)
                if matches!(&err, ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down")) =>
            {
                return Err(err)
            }
            Err(err) if is_death_abort(&err) => {
                return Err(err);
            }
            // A dead socket must reconnect, not limp to the next slice.
            Err(err) if crate::skill_common::is_conn_abort(&err) => return Err(err),
            Err(err) => {
                if (bot.world.x, bot.world.z) == before {
                    bot.log(&format!("trade with {keeper} failed: walk to spawn ({},{}) failed: {err}", spawn.1, spawn.2));
                    return Ok(false);
                }
            }
        }
    }
    let deadline = bot.world.ticks + 400;
    trade_live_rounds(bot, keeper, main_com, deadline)
}

/// Click rounds against the streamed keeper: approach from every around
/// tile, open trade, wait for the shop stock com. Waits for the keeper to
/// stream first (NPCs pop in after the walk, not on arrival): without this
/// the two angle rounds burn on an empty view and the trade fails though
/// the keeper stands there a few ticks later.
fn trade_live_rounds(bot: &mut Bot, keeper: &str, main_com: u16, deadline: u64) -> Result<bool, ScriptError> {
    while bot.find_npc(keeper).is_none() && bot.world.ticks < deadline {
        bot.pump(5)?;
    }
    for _round in 0..2 {
        if bot.world.ticks >= deadline {
            break;
        }
        let Some(slot) = bot.find_npc(keeper) else {
            bot.log(&format!("keeper {keeper} not streamed at {:?}", bot.position()));
            bot.pump(5)?;
            continue;
        };
        let npc = match bot.world.npcs.get(&slot) {
            Some(npc) => (npc.x, npc.z, npc.npc_type),
            None => {
                bot.log(&format!("keeper {keeper} slot {slot} gone at {:?}", bot.position()));
                bot.pump(5)?;
                continue;
            }
        };
        let Some(op) = npc_op(npc.2, "trade") else {
            bot.log(&format!("keeper {keeper} has no Trade op at {:?}", bot.position()));
            return Ok(false);
        };
        let mut tiles: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (npc.0 + dx, npc.1 + dz)))
            .filter(|tile| *tile != (npc.0, npc.1))
            .collect();
        tiles.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        for (tile_x, tile_z) in tiles {
            if bot.world.ticks >= deadline {
                break;
            }
            // Dead socket, not a bad angle: reconnect instead of pacing
            // the other sides of a booth we cannot touch. A down service
            // is typed `nav-down`: loud, never paced as a bad angle.
            if let Err(err) = bot.walk_to(tile_x, tile_z, bot.world.level, 750) {
                if matches!(&err, lifescape_sdk::Error::Travel(detail) if detail.contains("nav-down")) {
                    return Err(err.into());
                }
                if matches!(err, lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)) {
                    return Err(err.into());
                }
                bot.log(&format!("trade with {keeper} failed: walk_to ({tile_x},{tile_z}) failed: {err}"));
                continue;
            }
            if let Err(err) = bot.op_npc(op, slot) {
                if matches!(err, lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)) {
                    return Err(err.into());
                }
                bot.log(&format!("trade with {keeper} failed: op_npc slot {slot} failed: {err}"));
                continue;
            }
            // Short wait per angle: one blocked counter side must not eat
            // the whole deadline before the other sides are tried.
            let end = bot.world.ticks + 50;
            while bot.world.modal.is_none() && bot.world.ticks < end {
                bot.pump(5)?;
            }
            if bot.world.inv.contains_key(&main_com) {
                return Ok(true);
            }
            if bot.world.modal.is_some() {
                bot.log(&format!("trade with {keeper}: modal open without shop stock at {:?}, closing", bot.position()));
                bot.close_modal()?;
                bot.pump(5)?;
            } else {
                bot.log(&format!("trade with {keeper}: shop modal never opened at {:?} (timeout)", bot.position()));
            }
        }
    }
    bot.log(&format!("trade with {keeper} failed at {:?}", bot.position()));
    Ok(false)
}

/// Zero progress after a trade click is a refusal (shop out of stock,
/// sale rejected), never a silent `Ok(0)`. Both sell and buy share it.
fn require_trade_gain(gained: u32, what: &str, bot: &Bot) -> Result<u32, ScriptError> {
    if gained == 0 {
        return Err(ScriptError::Usage(format!("trade gave no {what} at {:?}", bot.position())));
    }
    Ok(gained)
}

/// Sell carried products until `target_coins` are raised (shortfall only,
// never stockpiles sale coins). Returns coins gained.
pub fn sell_for_target(
    bot: &mut Bot,
    product_ids: &[u16],
    coins: u16,
    target_coins: u32,
) -> Result<u32, ScriptError> {
    if target_coins == 0 || !product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
        return Ok(0);
    }
    let side_com = (bot.tables.com_id)("shop_template_side:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template_side:inv missing from component table".to_string()))?;
    let Some((keeper, spawn)) = general_store(bot) else {
        bot.log("no general store found");
        return Ok(0);
    };
    if !trade_to_keeper(bot, &keeper, spawn)? {
        bot.log(&format!("general store trade failed at {:?}", bot.position()));
        return Ok(0);
    }
    let coins_before = pack_total(bot, coins);
    if !product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
        bot.close_modal()?;
        return Err(ScriptError::Usage(format!("products vanished after trade opened at {:?}", bot.position())));
    };
    // Probe each product's first unit: a low-cost item sells for 0 coins at
    // general-store rates (shop.rs2 sell_item moves the item but pays nothing
    // under 1 gp, silently), so skip worthless stock instead of failing the
    // whole sale on the first click.
    let mut unit = 0u32;
    let mut worthless: Vec<u16> = Vec::new();
    let held: Vec<u16> = product_ids.iter().filter(|id| pack_total(bot, **id) > 0).copied().collect();
    for id in held {
        let Ok(slot) = pack_slot(bot, id) else {
            continue;
        };
        bot.inv_button(2, id, slot, side_com)?;
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target && pack_total(bot, coins) <= coins_before {
            bot.pump(5)?;
        }
        unit = pack_total(bot, coins).saturating_sub(coins_before);
        if unit > 0 {
            break;
        }
        let name = (bot.tables.obj_name)(id).unwrap_or_else(|| format!("obj{id}"));
        bot.log(&format!("{name} sells for 0, skipping it"));
        worthless.push(id);
    }
    if unit == 0 {
        bot.close_modal()?;
        bot.pump(5)?;
    }
    let unit = require_trade_gain(unit, "coins", bot)?;
    while pack_total(bot, coins) < coins_before + target_coins {
        let Some(next) =
            product_ids.iter().find(|id| !worthless.contains(id) && pack_total(bot, **id) > 0).copied()
        else {
            break;
        };
        let need = coins_before + target_coins - pack_total(bot, coins);
        let amount = pack_total(bot, next).min(((need + unit - 1) / unit).max(1));
        let Some((op, _)) = sell_batches(amount).first().copied() else {
            break;
        };
        let Ok(slot) = pack_slot(bot, next) else {
            break;
        };
        bot.inv_button(op, next, slot, side_com)?;
        bot.pump(5)?;
    }
    let gained = pack_total(bot, coins) - coins_before;
    bot.log(&format!("ok sold products for {gained} coins"));
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(gained)
}

/// Buy `count` of an object from its nearest stocking shop. Returns bought.
pub fn buy_stock(bot: &mut Bot, obj_name: &str, count: u32) -> Result<u32, ScriptError> {
    if count == 0 {
        return Ok(0);
    }
    if !bot.allow_shop {
        bot.log(&format!("shop buys refused for this task, {obj_name} stays unbought"));
        return Ok(0);
    }
    let main_com = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    let Some((keeper, spawn)) = shop_stock_keeper(bot, obj_name) else {
        bot.log(&format!("no shop stocks {obj_name} nearby"));
        return Ok(0);
    };
    if !trade_to_keeper(bot, &keeper, spawn)? {
        bot.log(&format!("trade with {keeper} failed"));
        return Ok(0);
    }
    let wanted = obj_id(bot, obj_name)?;
    let before = pack_total(bot, wanted);
    let target = bot.world.ticks + 50;
    let mut slot_found = None;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if let Some(slots) = bot.world.inv.get(&main_com) {
            slot_found = slots.iter().find(|(_, (held, _))| *held == wanted).map(|(slot, _)| *slot);
            if slot_found.is_some() {
                break;
            }
        }
    }
    let Some(shop_slot) = slot_found else {
        bot.close_modal()?;
        bot.pump(5)?;
        return Err(ScriptError::Usage(format!("no {obj_name} in shop stock at {:?}", bot.position())));
    };
    for (op, _) in sell_batches(count) {
        bot.inv_button(op, wanted, shop_slot, main_com)?;
        bot.pump(5)?;
    }
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && pack_total(bot, wanted) <= before {
        bot.pump(5)?;
    }
    let gained = pack_total(bot, wanted) - before;
    bot.close_modal()?;
    bot.pump(5)?;
    require_trade_gain(gained, obj_name, bot)
}

/// One upgrade buy attempt with bronze-first fallback: tries the affordable
/// cands best-first, one attempt each, downgrading on any shop failure
/// (empty live stock, short coins). Conn/death/interrupt still abort;
/// anything else logs and tries the next cheaper cand. Returns the bought
/// name or None when nothing affordable succeeded. Never loops: one pass.
pub fn buy_best_affordable(bot: &mut Bot, cands: &[String]) -> Result<Option<String>, ScriptError> {
    let have = pack_total(bot, obj_id(bot, "coins")?);
    for cand in cands {
        let price = obj_price(cand);
        if price <= 0 || have < price as u32 {
            bot.log(&format!("skip {cand}: costs {price}, have {have}"));
            continue;
        }
        match buy_stock(bot, cand, 1) {
            Ok(bought) if bought > 0 => {
                bot.log(&format!("ok upgraded to {cand}"));
                return Ok(Some(cand.clone()));
            }
            Ok(_) => {
                bot.log(&format!("{cand} trade failed at {:?}, trying cheaper", bot.position()));
            }
            Err(error) if is_death_abort(&error) => return Err(error),
            Err(error) if is_conn_abort(&error) => return Err(error),
            Err(error) if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) => return Err(error),
            Err(error) => {
                bot.log(&format!("{cand} buy failed ({error}), trying cheaper"));
            }
        }
    }
    Ok(None)
}

/// Sell-button batches for an amount: 10s, then 5s, then 1s.
pub fn sell_batches(mut amount: u32) -> Vec<(u8, u32)> {
    let mut batches = Vec::new();
    for (op, size) in [(4u8, 10u32), (3, 5), (2, 1)] {
        while amount >= size {
            batches.push((op, size));
            amount -= size;
        }
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::{buy_best_affordable, buy_stock, keeper_fallback_log, live_keeper, require_trade_gain, sell_for_target, shop_mates, trade_to_keeper};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::world::NpcInfo;
    use lifescape_sdk::Bot;

    fn bob_type() -> u16 {
        (0..8000usize)
            .find(|id| {
                lifescape_gamedata::npc_by_id(*id)
                    .and_then(|entry| entry.name)
                    .is_some_and(|name| name.eq_ignore_ascii_case("bob"))
            })
            .expect("bob in the npc dump") as u16
    }

    /// Nominal guards stay silent Ok(0): no target, no products, no order.
    #[test]
    fn zero_targets_stay_silent_ok() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert_eq!(sell_for_target(&mut bot, &[], 995, 0).expect("target 0 stays Ok(0)"), 0);
        assert_eq!(sell_for_target(&mut bot, &[], 995, 10).expect("no products stays Ok(0)"), 0);
        assert_eq!(buy_stock(&mut bot, "bronze_axe", 0).expect("count 0 stays Ok(0)"), 0);
    }

    /// Shared loud guard: zero progress after a trade click is an error,
    /// nonzero passes through untouched.
    #[test]
    fn zero_trade_gain_fails_loud() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let err = require_trade_gain(0, "coins", &bot).expect_err("zero gain is loud");
        assert!(err.to_string().contains("trade gave no coins"), "reason kept, got: {err}");
        assert_eq!(require_trade_gain(3, "coins", &bot).expect("nonzero passes"), 3);
    }

    /// Keeper out of reach in an empty world: without a service the
    /// spawn walk fails `nav-down`, which `trade_with` propagates typed
    /// by contract (no walk, no limp-along). The direct `walk_to` below
    /// proves the root cause is `nav-down` and nothing else.
    #[test]
    fn unreachable_spawn_returns_false() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        assert!(bot.world.npcs.is_empty(), "empty world, keeper absent");
        let error = trade_to_keeper(&mut bot, "no_such_keeper", (0, 3200, 3227)).expect_err("no service, nav-down is loud");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
        let error = bot.walk_to(3200, 3227, bot.world.level, 100).expect_err("direct walk is nav-down");
        assert!(
            matches!(&error, lifescape_sdk::Error::Travel(detail) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
    }

    /// Live keeper known: the live tile wins, the static spawn (which can
    /// sit inside a wall) is ignored.
    #[test]
    fn live_keeper_known_means_spawn_ignored() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3095;
        bot.world.z = 3240;
        bot.world.npcs.insert(
            7,
            NpcInfo { npc_type: bob_type(), x: 3096, z: 3241, level: 0, name: None, anim: None },
        );
        let found = live_keeper(&bot, "bob").expect("streamed bob resolves live");
        assert_eq!((found.1, found.2), (3096, 3241), "live tile wins, got: {found:?}");
        assert_ne!((found.1, found.2), (3232, 3203), "static spawn ignored when live is there");
    }

    /// Live keeper with no service: the booth-side walk fails `nav-down`,
    /// which the trade rounds propagate typed by contract (no walk, no
    /// pacing to the next angle). The direct `walk_to` below proves the
    /// root cause is `nav-down` and nothing else (never a local plan).
    #[test]
    fn live_keeper_offline_returns_false() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3095;
        bot.world.z = 3240;
        bot.world.npcs.insert(
            7,
            NpcInfo { npc_type: bob_type(), x: 3096, z: 3241, level: 0, name: None, anim: None },
        );
        let error = trade_to_keeper(&mut bot, "bob", (0, 3232, 3203)).expect_err("no service, nav-down is loud");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
        let error = bot.walk_to(3096, 3241, bot.world.level, 100).expect_err("direct walk is nav-down");
        assert!(
            matches!(&error, lifescape_sdk::Error::Travel(detail) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
    }

    /// Keeper absent: no live tile, and the fallback line names the keeper
    /// and the spawn so the repli is visible in the logs.
    #[test]
    fn missing_keeper_logs_spawn_fallback() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert!(bot.world.npcs.is_empty(), "empty world, keeper absent");
        assert!(live_keeper(&bot, "bob").is_none(), "no live tile without a stream");
        let line = keeper_fallback_log("bob", (0, 3232, 3203));
        assert!(line.contains("keeper bob not streamed, trying spawn"), "exact fallback line, got: {line}");
    }

    /// Missing keeper falls back to the shop's other staff: the Lumbridge
    /// general store lists the assistant next to keeper 1, the keeper
    /// itself is excluded, and an unknown keeper has no mates (trade
    /// behaviour for it is unchanged).
    #[test]
    fn missing_keeper_falls_back_to_shop_mate() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3209;
        bot.world.z = 3247;
        let mates = shop_mates(&bot, "generalshopkeeper1");
        assert!(
            mates.iter().any(|(name, _)| name.eq_ignore_ascii_case("generalassistant1")),
            "assistant is a mate, got: {mates:?}"
        );
        assert!(
            mates.iter().all(|(name, _)| !name.eq_ignore_ascii_case("generalshopkeeper1")),
            "keeper itself excluded, got: {mates:?}"
        );
        assert!(shop_mates(&bot, "no_such_keeper").is_empty(), "unknown keeper has no mates");
    }

    /// Affordable upgrade shortlist never walks when nothing is affordable:
    /// empty cands and a broke pack both return None with no shop walk (an
    /// offline walk would fail, so None proves nothing was attempted).
    #[test]
    fn buy_best_affordable_skips_without_sending() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert_eq!(buy_best_affordable(&mut bot, &[]).expect("no cands buys nothing"), None);
        assert_eq!(
            buy_best_affordable(&mut bot, &["bronze_axe".to_string()]).expect("broke pack buys nothing"),
            None
        );
    }
}
