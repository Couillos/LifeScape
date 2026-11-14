//! Market flip: merchant engagement on the market script (D18).
//! Split from `market.rs` so both stay under the activity plafond.

#![forbid(unsafe_code)]

use lifescape_json::{str_value, Value};
use lifescape_sdk::Bot;

use crate::market_client::market_call;
use crate::kit::pack::obj_id;
use crate::{ScriptError, ScriptResult};

use super::market::tradeable_id;

/// `market flip [--budget-max N] [--node ID] [item] [qty]`: merchant
/// engagement (D18) on the one script every bot already runs. Prices off
/// the book (daemon `flip_quote`, 2026-11-14): buys at the best bid + 1
/// when the spread pays its time, sells a demand-sized lot of its stock
/// at the best ask - 1, never under what the stock cost plus that
/// margin (never at reference +/- 1), waits out the engagement
/// (market pauses still deliver inline), cancels the leftovers, exits on
/// budget so the session reports `budget_exhausted`. Engagement sans
/// objet: no item target, the clock ends it.
pub fn run_flip(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (budget_max, positionals) = parse_flip(args)?;
    let mut item = positionals.first().cloned().unwrap_or_else(|| "feather".to_string());
    let mut qty = 100i64;
    if let Some(raw) = positionals.get(1) {
        let parsed: i64 = raw.parse().map_err(|_| ScriptError::Usage("qty must be a number".to_string()))?;
        qty = parsed.max(1);
    }
    // No explicit item: flip the most valuable banked or carried F2P
    // tradeable stock (coins and untradeables never qualify). A merchant
    // sells what it holds; the brain only sets the engagement window.
    if positionals.is_empty() {
        if let Some((name, amount)) = richest_stock(bot) {
            item = name;
            qty = amount.min(qty);
            bot.log(&format!("flip auto-pick: {item} x{qty}"));
        }
    }
    let id = tradeable_id(&item)?;
    let quote = market_call("flip_quote", vec![("account", str_value(&bot.username)), ("item", Value::Int(i64::from(id))), ("qty", Value::Int(qty))])?;
    let legs = FlipLegs::from_reply(&quote).map_err(|error| ScriptError::Usage(format!("flip_quote {item}: {error}")))?;
    bot.log(&format!("flip {item}: buy {:?} sell {:?} (floor {:?}, margin {}, lot {})", legs.buy, legs.sell, legs.sell_floor, legs.margin, legs.lot));
    let coins = obj_id(bot, "coins")?;
    let pack_coins = bot.inventory().get(&coins).copied().unwrap_or(0) as i64;
    let buy_price = legs.buy.unwrap_or(0);
    let buy_qty = if buy_price >= 1 { qty.min(pack_coins / buy_price).max(0) } else { 0 };
    if buy_qty > 0 {
        let reply = market_call(
            "place",
            vec![
                ("account", str_value(&bot.username)),
                ("side", str_value("buy")),
                ("item", Value::Int(i64::from(id))),
                ("qty", Value::Int(buy_qty)),
                ("price", Value::Int(buy_price)),
            ],
        )?;
        bot.log(&format!("flip buy {buy_qty} x{item} @ {buy_price} ({})", reply.get("order").as_i64().unwrap_or(-1)));
    } else {
        bot.log("flip: no buy leg (spread under its margin, or no coins)");
    }
    let held = bot.inventory().get(&id).copied().unwrap_or(0) as i64;
    // Merchants trade their banked stock, not just what they carry: pull
    // the sell leg from the nearest bank when the pack falls short.
    let mut held = held;
    if held < qty {
        let banked = bot.bank_items().get(&id).copied().unwrap_or(0) as i64;
        if banked > 0 {
            let bank = crate::kit::bank::nearest_bank(bot)?;
            if bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
                let _ = bot.bank_withdraw(id, (qty - held).min(banked) as u32, 300);
                let _ = bot.close_modal();
                bot.pump(3)?;
            }
            held = bot.inventory().get(&id).copied().unwrap_or(0) as i64;
        }
    }
    // One demand-sized lot (no dumping), walking down to its floor.
    let lot = held.min(qty).min(legs.lot);
    if let (true, Some(sell_price), Some(floor)) = (lot > 0, legs.sell, legs.sell_floor) {
        let reply = market_call(
            "place",
            vec![
                ("account", str_value(&bot.username)),
                ("side", str_value("sell")),
                ("item", Value::Int(i64::from(id))),
                ("qty", Value::Int(lot)),
                ("price", Value::Int(sell_price)),
                ("floor", Value::Int(floor.min(sell_price))),
            ],
        )?;
        bot.log(&format!("flip sell {lot} x{item} @ {sell_price} floor {floor} ({})", reply.get("order").as_i64().unwrap_or(-1)));
    } else if held > 0 {
        bot.log(&format!("flip: holding {held} {item} (no own cost known or over the band)"));
    }
    let start_tick = bot.world.ticks;
    let start_pause = bot.pause_ticks;
    loop {
        bot.pump(5)?;
        let work = bot.world.ticks.saturating_sub(start_tick).saturating_sub(bot.pause_ticks.saturating_sub(start_pause));
        if work >= budget_max {
            break;
        }
    }
    cancel_open(bot)?;
    Ok(crate::kit::cycle::BUDGET_EXIT)
}

/// The daemon's flip legs for this merchant (`flip_quote`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlipLegs {
    pub buy: Option<i64>,
    pub sell: Option<i64>,
    pub sell_floor: Option<i64>,
    pub lot: i64,
    pub margin: i64,
}

impl FlipLegs {
    pub(crate) fn from_reply(reply: &Value) -> Result<FlipLegs, String> {
        if reply.get("ok").as_bool() != Some(true) {
            return Err(reply.get("error").as_str().unwrap_or("flip_quote refused").to_string());
        }
        Ok(FlipLegs {
            buy: reply.get("buy").as_i64(),
            sell: reply.get("sell").as_i64(),
            sell_floor: reply.get("sell_floor").as_i64(),
            lot: reply.get("lot").as_i64().unwrap_or(0),
            margin: reply.get("margin").as_i64().unwrap_or(0),
        })
    }
}

/// Most valuable banked or carried F2P tradeable stock (debug name,
/// capped quantity): what a merchant without orders flips. Coins never
/// qualify; untradeable and members-only stock never lists; worn gear
/// never lists either (selling it would strip the bot mid-engagement).
fn richest_stock(bot: &Bot) -> Option<(String, i64)> {
    let coins = obj_id(bot, "coins").ok()?;
    let mut worn: std::collections::HashSet<u16> = std::collections::HashSet::new();
    if let Some(com) = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME) {
        if let Some(slots) = bot.world.inv.get(&com) {
            for (held, _count) in slots.values() {
                worn.insert(*held);
            }
        }
    }
    let mut held: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
    for (id, count) in bot.inventory().iter() {
        *held.entry(*id).or_insert(0) += *count;
    }
    for (id, count) in bot.bank_items().iter() {
        *held.entry(*id).or_insert(0) += *count;
    }
    held.remove(&coins);
    for held_id in worn {
        held.remove(&held_id);
    }
    let mut best: Option<(String, i64, i64)> = None;
    for (id, count) in held {
        if count == 0 {
            continue;
        }
        let Some(entry) = lifescape_gamedata::obj_by_id(usize::from(id)) else {
            continue;
        };
        let Some(name) = entry.name else {
            continue;
        };
        if !entry.tradeable || entry.members == Some(true) {
            continue;
        }
        let value = entry.price.max(1) as i64 * count as i64;
        if best.as_ref().is_none_or(|(_, _, top)| value > *top) {
            best = Some((name.to_string(), count as i64, value));
        }
    }
    best.map(|(name, count, _)| (name, count))
}

/// Cancel every open order of this account. Best effort per order, loud
/// per failure: a stranded buy still fills later and the holdings check
/// counts it, never a silent loss.
fn cancel_open(bot: &mut Bot) -> ScriptResult {
    let reply = market_call("my_orders", vec![("account", str_value(&bot.username))])?;
    let mut cancelled = 0;
    for entry in reply.get("orders").as_arr().cloned().unwrap_or_default() {
        if entry.get("status").as_str() != Some("open") {
            continue;
        }
        let order = entry.get("order").as_i64().unwrap_or(-1);
        let done = market_call(
            "cancel",
            vec![("account", str_value(&bot.username)), ("order", Value::Int(order))],
        )?;
        if done.get("ok").as_bool() == Some(true) {
            cancelled += 1;
        } else {
            bot.log(&format!("flip: cancel {order} refused: {}", done.get("error").as_str().unwrap_or("?")));
        }
    }
    bot.log(&format!("flip: cancelled {cancelled} open orders"));
    Ok(0)
}


/// `market flip` arguments: the engagement window (`--budget-max`, 6000
/// by default) and the optional `[item] [qty]` positionals. Node id and
/// floor ride along for the session report.
pub(crate) fn parse_flip(args: &[String]) -> Result<(u64, Vec<String>), ScriptError> {
    let mut budget_max = 6000u64;
    let mut positionals: Vec<String> = Vec::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--budget-max" => {
                budget_max = args.get(index + 1).and_then(|raw| raw.parse().ok()).ok_or_else(|| ScriptError::Usage("flip --budget-max N".to_string()))?;
                index += 2;
            }
            "--node" | "--budget-min" => index += 2,
            other if other.starts_with("--") => return Err(ScriptError::Usage("market flip [--budget-max N] [--node ID] [item] [qty]".to_string())),
            _ => {
                positionals.push(args[index].clone());
                index += 1;
            }
        }
    }
    Ok((budget_max, positionals))
}

#[cfg(test)]
mod tests {
    use super::FlipLegs;
    use lifescape_json::{object, Value};

    #[test]
    fn flip_legs_read_the_daemon_quote() {
        let reply = object(vec![
            ("ok", Value::Bool(true)),
            ("buy", Value::Int(11)),
            ("sell", Value::Int(19)),
            ("sell_floor", Value::Int(14)),
            ("lot", Value::Int(25)),
            ("margin", Value::Int(3)),
        ]);
        assert_eq!(FlipLegs::from_reply(&reply), Ok(FlipLegs { buy: Some(11), sell: Some(19), sell_floor: Some(14), lot: 25, margin: 3 }));
        let held = object(vec![("ok", Value::Bool(true)), ("buy", Value::Null), ("sell", Value::Null), ("lot", Value::Int(10)), ("margin", Value::Int(2))]);
        let legs = FlipLegs::from_reply(&held).expect("parses");
        assert_eq!((legs.buy, legs.sell), (None, None), "no leg: nothing placed, never ref +/- 1");
        assert!(FlipLegs::from_reply(&object(vec![("ok", Value::Bool(false))])).is_err());
    }
}
