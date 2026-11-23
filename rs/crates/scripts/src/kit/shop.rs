//! Shop trade for the skill scripts: keeper lookup, loud trade clicks,
//! selling produce and buying stock. Absorbed from the old `skill_trade`
//! layer (D61): the kit owns it now.
//! Every wait is tick-bounded (`bot.world.ticks`); nothing sleeps. Trade
//! failures log their reason; post-trade sale/purchase failures are loud
//! errors, never silent `Ok(0)`.

#![forbid(unsafe_code)]


use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, obj_price, pack_total};
use crate::kit::survival::{is_conn_abort, is_death_abort};
use crate::skill_walk::walk_checked;
use crate::ScriptError;

/// Nearest-shop trek range: keeper spawns past this never count.
const TREK_LIMIT: i64 = 400;

/// Nearest shop stocking `obj_name` (stock count > 0) with its keeper spawn.
// shortcut: keeper NPC op for Trade is assumed to be op 2 (talk/trade op
// order is pack data, not in the gamedata dump); fails loudly otherwise.
pub fn shop_stock_keeper(bot: &Bot, obj_name: &str) -> Option<(String, (i32, i32, i32))> {
    keeper_within(bot, obj_name, None)
}

/// Keepers a buy may walk to on its own, Wilderness included (user
/// 2026-11-16: open to every bot): never underground (z ≥ 6400) except
/// on a floor the nav routes
/// (`access::shop_floor_routed`: the Dwarven Mine, Nurmof's pickaxes),
/// never across the toll fence `fence` (the brain passes `toll` when it
/// funded the gate).
pub fn reachable_keeper(spawn: (i64, i64, i64), bot: (i32, i32), fence: Option<i32>) -> bool {
    let (x, z, level) = spawn;
    let open = z < 6400 || knowledge::access::shop_floor_routed(x, z, level);
    open && !crate::kit::bans::across_toll(bot, (x as i32, z as i32), fence)
}

/// `shop_stock_keeper` with the toll fence line (`None` = unfenced).
pub fn keeper_within(bot: &Bot, obj_name: &str, fence: Option<i32>) -> Option<(String, (i32, i32, i32))> {
    keeper_where(bot, obj_name, fence, None)
}

/// Nearest keeper of a stocking shop, or of the named shop only (the
/// planner's `--shop`: its quest gate was judged by the brain, so a
/// gated shop the bot opened counts; a dump stock of 0 restocks live).
fn keeper_where(bot: &Bot, obj_name: &str, fence: Option<i32>, named: Option<&str>) -> Option<(String, (i32, i32, i32))> {
    let mut best: Option<(i64, (String, (i32, i32, i32)))> = None;
    // Only shops an NPC opens on an F2P world without a quest gate
    // (`f2p_shop_access`): Zaff's staffs count, Shantay's pass never.
    for shop in knowledge::f2p::f2p_shops() {
        if named.is_some_and(|key| key != shop.name) {
            continue;
        }
        if !knowledge::f2p::f2p_stock(&shop).iter().any(|stock| stock.obj == obj_name && (stock.count > 0 || named.is_some())) {
            continue;
        }
        let Some(access) = knowledge::f2p::f2p_shop_access(&shop.name) else { continue };
        if access.quest.is_some() && named.is_none() {
            continue;
        }
        for npc_id in access.npcs {
            let Some(entry) = knowledge::npc_by_id(npc_id) else { continue };
            let Some(keeper_name) = entry.name else { continue };
            for spawn in entry.spawns {
                if spawn.level != i64::from(bot.world.level) || !reachable_keeper((spawn.x, spawn.z, spawn.level), (bot.world.x, bot.world.z), fence) {
                    continue;
                }
                let dist = (spawn.x - i64::from(bot.world.x)).abs() + (spawn.z - i64::from(bot.world.z)).abs();
                if dist > TREK_LIMIT {
                    continue;
                }
                if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                    best = Some((dist, (keeper_name.to_string(), (spawn.level as i32, spawn.x as i32, spawn.z as i32))));
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
            Err(err) if crate::kit::survival::is_conn_abort(&err) => return Err(err),
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

/// Ticks a service NPC gets to stream in once the bot stands at its spawn
/// (NPCs pop in after the walk, not on arrival).
pub const NPC_STREAM_TICKS: u64 = 20;

/// Where to walk before talking to `name`: its spawn when it is not in
/// view, nothing when it streams already.
pub fn spawn_walk(bot: &Bot, name: &str, spawn: (i32, i32)) -> Option<(i32, i32)> {
    match bot.find_npc(name) {
        Some(_) => None,
        None => Some(spawn),
    }
}

/// Slot of a service NPC (Aubury...): walk to its spawn when out of view,
/// then wait for it to stream. Fails typed when the spawn is unreachable
/// or the NPC never shows (`no <name> near (x,z,level)`).
pub fn npc_at_spawn(bot: &mut Bot, name: &str, spawn: (i32, i32), walk_ticks: u64) -> Result<u16, ScriptError> {
    if let Some((x, z)) = spawn_walk(bot, name, spawn) {
        bot.log(&format!("walking to {name} at ({x},{z})"));
        bot.intend(&format!("walking to {name}"));
        if !walk_checked(bot, x, z, walk_ticks)? {
            return Err(ScriptError::Failed(format!("{name} spawn ({x},{z}) unreachable from {:?}", bot.position())));
        }
        let deadline = bot.world.ticks + NPC_STREAM_TICKS;
        while bot.find_npc(name).is_none() && bot.world.ticks < deadline {
            bot.pump(2)?;
        }
    }
    bot.find_npc(name).ok_or_else(|| ScriptError::Failed(format!("no {name} near {:?}", bot.position())))
}

/// Click rounds against the streamed keeper: approach from every around
/// tile, open trade, wait for the shop stock com. Waits for the keeper to
/// stream first (NPCs pop in after the walk, not on arrival): without this
/// the two angle rounds burn on an empty view and the trade fails though
/// the keeper stands there a few ticks later.
/// A modal other than an overlay (the Wilderness level banner is an
/// informational overlay, never the keeper's answer).
fn blocking_modal(bot: &Bot) -> bool {
    bot.world.modal.as_ref().is_some_and(|modal| modal.layer != "overlay")
}

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
            // Only this keeper's stock counts: a shop opened earlier in the
            // session leaves its inventory behind (live admin 2026-11-16,
            // Zambo's stock read as Fat Tony's: "no pizza_base in shop").
            bot.world.inv.remove(&main_com);
            if let Err(err) = bot.op_npc(op, slot) {
                if matches!(err, lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)) {
                    return Err(err.into());
                }
                bot.log(&format!("trade with {keeper} failed: op_npc slot {slot} failed: {err}"));
                continue;
            }
            // Short wait per angle: one blocked counter side must not eat
            // the whole deadline before the other sides are tried. The
            // Wilderness level overlay is no answer to the trade.
            let end = bot.world.ticks + 50;
            while !bot.world.inv.contains_key(&main_com) && !blocking_modal(bot) && bot.world.ticks < end {
                bot.pump(5)?;
            }
            if bot.world.inv.contains_key(&main_com) {
                return Ok(true);
            }
            if blocking_modal(bot) {
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
        return Err(ScriptError::Failed(format!("trade gave no {what} at {:?}", bot.position())));
    }
    Ok(gained)
}

/// Buy `count` of an object from its nearest stocking shop. Returns bought.
pub fn buy_stock(bot: &mut Bot, obj_name: &str, count: u32) -> Result<u32, ScriptError> {
    buy_stock_at(bot, obj_name, count, None)
}

/// `buy_stock` at the named shop only (`gamedata::shops` key, the
/// planner's `--shop`): never another shop. A key no F2P shop carries, or
/// whose stock lacks the object, is a contract bug (`Usage`).
pub fn buy_stock_at(bot: &mut Bot, obj_name: &str, count: u32, shop: Option<&str>) -> Result<u32, ScriptError> {
    if let Some(key) = shop {
        let stocked = knowledge::f2p::f2p_shops().into_iter().find(|entry| entry.name == key).is_some_and(|entry| knowledge::f2p::f2p_stock(&entry).iter().any(|stock| stock.obj == obj_name));
        if !stocked {
            return Err(ScriptError::Usage(format!("no F2P shop {key:?} stocks {obj_name}")));
        }
    }
    if count == 0 {
        return Ok(0);
    }
    if !bot.allow_shop {
        bot.log(&format!("shop buys refused for this task, {obj_name} stays unbought"));
        return Ok(0);
    }
    // Merchant purchases (user 2026-11-15): every F2P object a keeper
    // stocks may be bought; a members object never (typed block, the
    // brain files another way).
    if !knowledge::obtain::shop_buy_allowed(obj_name) {
        bot.log(&format!("merchant purchase of {obj_name} refused: no F2P merchant purchase"));
        return Err(ScriptError::Blocked { missing: vec![obj_name.to_string()] });
    }
    let main_com = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    // Never past the toll fence or underground on the buy's own
    // initiative: the brain passes `toll` when it funds it.
    let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(bot) };
    let Some((keeper, spawn)) = keeper_where(bot, obj_name, fence, shop) else {
        bot.log(&format!("no reachable keeper of {} stocks {obj_name} (no underground or toll)", shop.unwrap_or("any shop")));
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
            slot_found = slots.iter().find(|(_, (held, _))| *held == wanted).map(|(slot, (_, stock))| (*slot, *stock));
            if slot_found.is_some() {
                break;
            }
        }
    }
    let shop_slot = match slot_found {
        Some((slot, stock)) if stock > 0 => slot,
        _ => {
            // The keeper stocks it (checked above) and the live stock is
            // empty: other buyers took it, the restock brings it back.
            // Contention, never a broken method (`buy_item` checks the
            // price before the stock: at stock 0 an adamant chainbody
            // costs 4 848 and the refusal reads "You don't have enough
            // coins", live 2026-11-17 jane_95, three counted failures).
            bot.close_modal()?;
            bot.pump(5)?;
            bot.log(&format!("{obj_name}: shop stock empty at {:?}, contended", bot.position()));
            let (x, z, level) = bot.assigned_site.unwrap_or((bot.world.x, bot.world.z, bot.world.level));
            return Err(ScriptError::Blocked { missing: vec![format!("{}{x},{z},{level}", crate::kit::site::CONTENDED_TAG)] });
        }
    };
    // The server's refusal ends the clicks (live 2026-11-21: 65 Buy-10
    // clicks, 65 « You don't have enough coins », Yogosun, ida_reed).
    let seen = bot.world.messages.len();
    for (op, _) in sell_batches(count) {
        bot.inv_button(op, wanted, shop_slot, main_com)?;
        bot.pump(5)?;
        if shop_refusal(&bot.world.messages, seen).is_some() {
            break;
        }
    }
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && pack_total(bot, wanted) <= before && shop_refusal(&bot.world.messages, seen).is_none() {
        bot.pump(5)?;
    }
    let gained = pack_total(bot, wanted).saturating_sub(before);
    bot.close_modal()?;
    bot.pump(5)?;
    let refusal = shop_refusal(&bot.world.messages, seen);
    if gained == 0 && refusal == Some(ShopRefusal::Stock) {
        // Bought out between the stock read and the click: contention.
        bot.log(&format!("{obj_name}: the shop ran out of stock at {:?}, contended", bot.position()));
        let (x, z, level) = bot.assigned_site.unwrap_or((bot.world.x, bot.world.z, bot.world.level));
        return Err(ScriptError::Blocked { missing: vec![format!("{}{x},{z},{level}", crate::kit::site::CONTENDED_TAG)] });
    }
    if gained == 0 {
        if let Some(refusal) = refusal {
            return Err(ScriptError::Failed(format!("trade gave no {obj_name} at {:?}: {}", bot.position(), refusal.line())));
        }
    }
    require_trade_gain(gained, obj_name, bot)
}

/// A shop's refusal of a buy (`shop.rs2` `buy_item`): every one ends the
/// purchase on the server side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopRefusal {
    /// « You don't have enough coins. »
    Coins,
    /// « You don't have enough inventory space. »
    Room,
    /// « The shop has run out of stock. »
    Stock,
}

impl ShopRefusal {
    /// The server's line, for the error text.
    pub fn line(self) -> &'static str {
        match self {
            ShopRefusal::Coins => "You don't have enough coins.",
            ShopRefusal::Room => "You don't have enough inventory space.",
            ShopRefusal::Stock => "The shop has run out of stock.",
        }
    }
}

/// Pure: the first shop refusal among the game messages since `seen`.
pub fn shop_refusal(messages: &[String], seen: usize) -> Option<ShopRefusal> {
    messages.get(seen..).unwrap_or(&[]).iter().find_map(|message| {
        [ShopRefusal::Coins, ShopRefusal::Room, ShopRefusal::Stock].into_iter().find(|refusal| message.contains(refusal.line()))
    })
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

/// Ticks one sale click gets to take its units from the pack.
pub const SELL_UNIT_TICKS: u64 = 10;

/// Sell button of the shop's side inventory for the biggest batch that
/// fits `units`: (button, size). `shop.rs2`: `inv_button2` sells 1,
/// `inv_button3` 5, `inv_button4` 10; a shop has no sell X in rev 274.
pub fn sell_batch_for(units: u32) -> Option<(u8, u32)> {
    [(4u8, 10u32), (3, 5), (2, 1)].into_iter().find(|(_, size)| *size <= units)
}

/// Units out of `count` the shop pays at least `min_unit` each for, from
/// its live stock `shop_holds`: the server values unit i of a sale at the
/// stock plus the i units before it (`shop.rs2` `sell_item`), bounded
/// from below (`shop_sale::unit_value_floor`); the count stops at the
/// first unit under. `None` when the shop buys no such object.
pub fn units_paying(shop: &str, obj_name: &str, shop_holds: i64, count: u32, min_unit: i64) -> Option<u32> {
    let mut paying = 0u32;
    while paying < count {
        if knowledge::shop_sale::unit_value_floor(shop, obj_name, shop_holds + i64::from(paying))? < min_unit {
            break;
        }
        paying += 1;
    }
    Some(paying)
}

/// Sells up to `count` units of `obj_name` (carried as `form`: the object
/// or its banknote) to the OPEN shop `shop` (`gamedata::shops` key) with
/// the sell buttons (user 2026-11-19: 10, 5, 1, never one click per unit
/// when a batch pays), only the units whose server value is at least
/// `min_unit` coins: never a unit under what the market pays (plan marché
/// Q3). The shop's stock is read again before each batch (other sellers
/// lower the price meanwhile). Returns (units sold, coins received). A
/// click the server refuses ends the sale, logged.
pub fn sell_units(bot: &mut Bot, form: u16, obj_name: &str, shop: &str, count: u32, min_unit: i64) -> Result<(u32, i64), ScriptError> {
    crate::kit::shop_reopen::sell_units_stop(bot, form, obj_name, shop, count, min_unit).map(|(sold, earned, _)| (sold, earned))
}

/// Pure: units one sale batch sold, read from the pack (`units_before`,
/// `units_after`). `None` when a market pause closed meanwhile
/// (`pauses_before` / `pauses_after`): the pause banked the pack, the
/// units gone are no sale (lot 8 re-review: the whole pile read as sold,
/// the coins negative); the caller stops and resumes from the bank.
pub fn batch_sold(units_before: u32, units_after: u32, pauses_before: u64, pauses_after: u64) -> Option<u32> {
    (pauses_after == pauses_before).then(|| units_before.saturating_sub(units_after))
}

/// Buy-button batches for an amount on the shop's own inventory: 10s,
/// then 5s, then 1s (`inv_button4/3/2,shop_template:inv`).
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
    use super::{buy_best_affordable, buy_stock, keeper_fallback_log, live_keeper, require_trade_gain, shop_mates, trade_to_keeper};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::world::NpcInfo;
    use lifescape_sdk::Bot;

    /// Live 2026-11-21 (Yogosun, ida_reed: 65 Buy-10 clicks on Aubury's
    /// earth runes, 65 « You don't have enough coins »): the server's
    /// refusal (`shop.rs2` `buy_item`) ends the clicks and names the
    /// cause; only the lines since the clicks count.
    #[test]
    fn a_shop_refusal_ends_the_buy_and_names_its_cause() {
        use super::{shop_refusal, ShopRefusal};
        let lines = |texts: &[&str]| texts.iter().map(|text| text.to_string()).collect::<Vec<String>>();
        assert_eq!(shop_refusal(&lines(&["Welcome!", "You don't have enough coins."]), 0), Some(ShopRefusal::Coins));
        assert_eq!(shop_refusal(&lines(&["You don't have enough inventory space."]), 0), Some(ShopRefusal::Room));
        assert_eq!(shop_refusal(&lines(&["The shop has run out of stock."]), 0), Some(ShopRefusal::Stock));
        assert_eq!(shop_refusal(&lines(&["You don't have enough coins.", "Harpoon: currently costs 5gp."]), 1), None, "an older refusal is not this buy's");
        assert_eq!(shop_refusal(&lines(&[]), 0), None);
    }

    /// Lot 8 re-review: a pause in a sale batch's wait banks the pack; the
    /// units gone are never counted as sold (no negative coins, the lot's
    /// rest resumes from the bank).
    #[test]
    fn a_batch_cut_by_a_pause_sells_nothing() {
        assert_eq!(super::batch_sold(158, 148, 4, 4), Some(10), "a plain batch");
        assert_eq!(super::batch_sold(158, 0, 4, 5), None, "the pause banked the pack");
        assert_eq!(super::batch_sold(158, 158, 4, 4), Some(0), "the shop took nothing");
    }

    fn bob_type() -> u16 {
        (0..8000usize)
            .find(|id| {
                lifescape_gamedata::npc_by_id(*id)
                    .and_then(|entry| entry.name)
                    .is_some_and(|name| name.eq_ignore_ascii_case("bob"))
            })
            .expect("bob in the npc dump") as u16
    }

    /// User 2026-11-19 (« on doit pas vendre clic par clic, mais en
    /// utilisant les options »): the biggest sell button the paying units
    /// allow, 10 then 5 then 1 (`shop.rs2`: no sell X on a shop in rev
    /// 274). thorn_vera's 28 yew logs: six clicks instead of 28.
    #[test]
    fn a_sale_clicks_the_biggest_paying_batch() {
        use super::sell_batch_for;
        assert_eq!(sell_batch_for(28), Some((4, 10)));
        assert_eq!(sell_batch_for(7), Some((3, 5)));
        assert_eq!(sell_batch_for(4), Some((2, 1)));
        assert_eq!(sell_batch_for(0), None);
        let (mut left, mut clicks) = (28u32, 0);
        while let Some((_, size)) = sell_batch_for(left) {
            left -= size;
            clicks += 1;
        }
        assert_eq!(clicks, 6);
    }

    /// The batch never passes the market's price: only the units whose
    /// server value (`unit_value_floor` at the shop's stock plus the units
    /// sold before them) pays at least `min_unit` are counted.
    #[test]
    fn only_the_units_paying_the_market_are_counted() {
        use super::units_paying;
        use lifescape_gamedata::shop_sale::unit_value_floor;
        let floor = |held: i64| unit_value_floor("generalshop1", "yew_logs", held).expect("a general store buys yew");
        let min_unit = floor(3);
        let paying = units_paying("generalshop1", "yew_logs", 0, 28, min_unit).expect("buys");
        assert!(paying >= 4, "{paying}");
        assert!((0..i64::from(paying)).all(|held| floor(held) >= min_unit));
        assert!(paying == 28 || floor(i64::from(paying)) < min_unit, "stops at the first unit under the market");
        assert_eq!(units_paying("generalshop1", "yew_logs", 0, 2, 1), Some(2), "never past the lot");
        assert_eq!(units_paying("generalshop1", "yew_logs", 0, 28, floor(0) + 1), Some(0), "the first unit already under");
        assert_eq!(units_paying("scimitarshop", "yew_logs", 0, 28, 1), None, "a shop that buys none");
    }

    /// Nominal guards stay silent Ok(0): no target, no products, no order.
    #[test]
    fn zero_targets_stay_silent_ok() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert_eq!(buy_stock(&mut bot, "bronze_axe", 0).expect("count 0 stays Ok(0)"), 0);
    }

    /// The Wilderness level overlay never reads as the keeper's answer; a
    /// main or chat modal does (live admin 2026-11-16, Fat Tony).
    #[test]
    fn the_wilderness_overlay_is_no_trade_answer() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        assert!(!super::blocking_modal(&bot));
        bot.world.modal = Some(lifescape_sdk::world::Modal { layer: "overlay".to_string(), interface_id: 197 });
        assert!(!super::blocking_modal(&bot), "the level banner");
        bot.world.modal = Some(lifescape_sdk::world::Modal { layer: "chat".to_string(), interface_id: 968 });
        assert!(super::blocking_modal(&bot));
    }

    /// A buy never walks underground or across the toll fence on its own
    /// (audit C10); `toll` lifts only the fence; the Wilderness is open
    /// (user 2026-11-16).
    #[test]
    fn keepers_stay_on_reachable_ground() {
        assert!(super::reachable_keeper((3212, 3246, 0), (3222, 3218), Some(3268)), "Lumbridge store");
        assert!(!super::reachable_keeper((3315, 3180, 0), (3222, 3218), Some(3268)), "Al Kharid across the fence");
        assert!(super::reachable_keeper((3315, 3180, 0), (3222, 3218), None), "toll paid: fence lifted");
        assert!(super::reachable_keeper((3080, 3930, 0), (3222, 3218), None), "the Wilderness is open");
        assert!(!super::reachable_keeper((3110, 9850, 0), (3222, 3218), None), "underground never");
        // 2026-11-14: Nurmof (the only F2P pickaxe upgrade seller) stands
        // on the Dwarven Mine floor the nav routes (Falador trapdoor).
        assert!(super::reachable_keeper((2998, 9844, 0), (3018, 9850), None), "routed shop floor");
    }

    /// User 2026-11-15: every F2P object a keeper stocks may be bought
    /// (the producible bronze sword at Varrock's sword shop too); a
    /// members object is refused typed `Blocked`, before any walk.
    #[test]
    fn members_objects_never_buy_from_merchants() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        match buy_stock(&mut bot, "abyssal_whip", 1) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["abyssal_whip".to_string()]),
            other => panic!("a members object must block, got {other:?}"),
        }
        assert!(lifescape_gamedata::obtain::shop_buy_allowed("bronze_sword"), "producible and stocked: allowed");
        assert!(lifescape_gamedata::obtain::shop_buy_allowed("bronze_axe"), "kit axe: allowed");
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
            NpcInfo { npc_type: bob_type(), x: 3096, z: 3241, level: 0, name: None, anim: None, face_entity: None, hit_tick: None, spotanim: None },
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
        bot.world.x = 3088;
        bot.world.z = 3240;
        bot.world.npcs.insert(
            7,
            NpcInfo { npc_type: bob_type(), x: 3096, z: 3241, level: 0, name: None, anim: None, face_entity: None, hit_tick: None, spotanim: None },
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
