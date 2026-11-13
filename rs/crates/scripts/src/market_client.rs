//! Worker-side market socket client: one JSON-lines round-trip per call.
//! Workers never touch the store file; the daemon owns it.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use lifescape_json::{object, str_value, Value};

use lifescape_sdk::Bot;

use crate::kit::bank::nearest_bank;
use crate::ScriptError;

/// One open deal row for this account (counterparty, terms).
#[derive(Debug, Clone, PartialEq)]
pub struct DealRow {
    pub id: i64,
    pub partner: String,
    pub buying: bool,
    pub item: u16,
    pub qty: u32,
    pub price: i64,
}

/// Open deals of one account (buyer, seller, item, qty, price).
pub fn my_deals(account: &str) -> Result<Vec<DealRow>, ScriptError> {
    let reply = market_call("my_deals", vec![("account", str_value(account))])?;
    let mut rows = Vec::new();
    for entry in reply.get("deals").as_arr().cloned().unwrap_or_default() {
        let buyer = entry.get("buyer").as_str().unwrap_or("").to_string();
        let seller = entry.get("seller").as_str().unwrap_or("").to_string();
        if buyer != account && seller != account {
            continue;
        }
        let buying = buyer == account;
        rows.push(DealRow {
            id: entry.get("deal").as_i64().unwrap_or(-1),
            partner: if buying { seller } else { buyer },
            buying,
            item: entry.get("item").as_i64().unwrap_or(-1) as u16,
            qty: entry.get("qty").as_i64().unwrap_or(0).max(0) as u32,
            price: entry.get("price").as_i64().unwrap_or(0),
        });
    }
    Ok(rows)
}

/// Physical id both sides trade: the banknote when one exists (bulky
/// stacks ride one slot), else the base object. Deterministic from the
/// deal item, so both bots agree without communicating.
pub fn trade_id(base: u16) -> u16 {
    let name = lifescape_gamedata::obj_by_id(usize::from(base)).and_then(|entry| entry.name).unwrap_or_default();
    if !name.is_empty() && !name.starts_with("cert_") {
        if let Some(note) = lifescape_gamedata::objs_by_name(&format!("cert_{name}")).into_iter().next() {
            return note.obj_id as u16;
        }
    }
    base
}

/// Typed deal report: trading settled, or failed with its reason.
pub fn deal_report(account: &str, id: i64, outcome: &str, reason: &str) {
    let _ = market_call(
        "deal_report",
        vec![
            ("account", str_value(account)),
            ("deal", Value::Int(id)),
            ("outcome", str_value(outcome)),
            ("reason", str_value(reason)),
        ],
    );
}

/// One market request, one reply. Transport failures are loud IO errors,
/// never empty books.
pub fn market_call(op: &str, params: Vec<(&str, Value)>) -> Result<Value, ScriptError> {
    let mut pairs = vec![("op", str_value(op))];
    pairs.extend(params);
    let mut body = lifescape_json::stringify(&object(pairs));
    body.push('\n');
    let sock = std::path::Path::new("run/market.sock");
    let stream = UnixStream::connect(sock)
        .map_err(|error| ScriptError::Sdk(lifescape_sdk::Error::Io(format!("market unreachable: {error}"))))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| ScriptError::Sdk(lifescape_sdk::Error::Io(error.to_string())))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| ScriptError::Sdk(lifescape_sdk::Error::Io(error.to_string())))?;
    (&stream)
        .write_all(body.as_bytes())
        .map_err(|error| ScriptError::Sdk(lifescape_sdk::Error::Io(error.to_string())))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|error| ScriptError::Sdk(lifescape_sdk::Error::Io(error.to_string())))?;
    lifescape_json::parse(&line).map_err(|error| ScriptError::Sdk(lifescape_sdk::Error::Io(format!("bad market reply: {error}"))))
}

/// Withdraw the trade lots when the pack falls short: notes for the
/// seller (one slot), coins for the buyer. True when carrying the lots.
pub fn ensure_goods(bot: &mut Bot, row: &DealRow, trade: u16, coins: u16) -> Result<bool, ScriptError> {
    if row.buying {
        let need_n = (i64::from(row.qty) * row.price) as u32;
        if bot.inventory().get(&coins).copied().unwrap_or(0) >= need_n {
            return Ok(true);
        }
    } else if bot.inventory().get(&row.item).copied().unwrap_or(0) >= row.qty
        || bot.inventory().get(&trade).copied().unwrap_or(0) >= row.qty
    {
        // The seller offers the carried form (base or notes); the buyer
        // adapts to whichever lands on the window.
        return Ok(true);
    }
    let bank = nearest_bank(bot)?;
    if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
        bot.log("bank did not open for the handover");
        return Ok(false);
    }
    // The bank pack streams a few ticks after the interface opens: wait
    // for actual slots before reading stock, or every form check misfires
    // on a present-but-empty container.
    let main_com = (bot.tables.com_id)("bank_main:inv");
    let streamed = bot.world.ticks + 60;
    while bot.world.ticks < streamed {
        if main_com.is_some_and(|com| bot.world.inv.get(&com).is_some_and(|slots| !slots.is_empty())) {
            break;
        }
        bot.pump(2)?;
    }
    // Empty the pack before withdrawing the lots (user rule: both sides
    // bank their goods at the nearest bank first, so nothing of the trade
    // rides along and a death on the road costs nothing). Live
    // 2026-11-12: b5xntyz6 carried 99 cow hides and 99 raw beef from its
    // melee run, so every `bank_withdraw` answered "You don't have enough
    // inventory space.", it walked back empty, and the buyer waiting at
    // the fountain lost the deal to `no_show` — three times in a row.
    if let Some(side_com) = (bot.tables.com_id)("bank_side:inv") {
        let _ = bot.bank_deposit_all(side_com, &[], 300);
        bot.pump(2)?;
    }
    if row.buying {
        let need_n = (i64::from(row.qty) * row.price) as u32;
        bot.bank_withdraw(coins, need_n, 300)?;
    } else if trade != row.item {
        // Withdraw whichever form the bank holds (base preferred, notes
        // otherwise): the buyer takes either form at the window.
        let banked = bot.bank_items();
        if banked.get(&row.item).copied().unwrap_or(0) >= row.qty {
            bot.bank_withdraw(row.item, row.qty, 300)?;
        } else {
            bot.bank_withdraw_noted(trade, row.qty, 300)?;
        }
    } else {
        bot.bank_withdraw(row.item, row.qty, 300)?;
    }
    let _ = bot.close_modal();
    bot.pump(3)?;
    let carried = if row.buying {
        bot.inventory().get(&coins).copied().unwrap_or(0) >= (i64::from(row.qty) * row.price) as u32
    } else {
        bot.inventory().get(&row.item).copied().unwrap_or(0) >= row.qty || bot.inventory().get(&trade).copied().unwrap_or(0) >= row.qty
    };
    if !carried {
        // Loud: a silent false sent the partner to the square alone.
        bot.log(&format!(
            "handover: lots never landed ({} {} x{}) at {:?}, the bank is short",
            if row.buying { "coins for" } else { "item" },
            row.item,
            row.qty,
            bot.position()
        ));
    }
    Ok(carried)
}

/// The seller's offered form: base when carried, else notes. The buyer
/// takes whatever lands on the window (validated below), so bulky stacks
/// ride one slot without forcing a bank dance on small ones.
pub fn seller_form(bot: &Bot, row: &DealRow, trade: u16) -> Option<(u16, u32)> {
    if bot.inventory().get(&row.item).copied().unwrap_or(0) >= row.qty {
        return Some((row.item, row.qty));
    }
    if bot.inventory().get(&trade).copied().unwrap_or(0) >= row.qty {
        return Some((trade, row.qty));
    }
    None
}

/// Acceptable seller lots from the buyer's side: the deal item in the
/// carried form (base or its banknote) at the exact quantity, no coins
/// mixed in. Anything else is a mismatch, never an accept.
pub fn buyer_want(offered: &std::collections::HashMap<u16, u32>, base: u16, trade: u16, qty: u32) -> Option<Vec<(u16, u32)>> {
    for candidate in [base, trade] {
        if offered.get(&candidate).copied().unwrap_or(0) == qty && offered.len() == 1 {
            return Some(vec![(candidate, qty)]);
        }
    }
    None
}

/// Surplus sell leg (PRD3 lot 3): one order opening at the market (10%
/// over the reference, or the best bid when richer) that walks down to
/// `floor` while it rests. Returns whether the carnet took it. A refusal
/// logs and passes: the next pass re-lists.
pub fn place_sell(bot: &mut Bot, item: u16, qty: i64, floor: i64) -> Result<bool, ScriptError> {
    let quote = market_call("quote", vec![("item", Value::Int(i64::from(item)))])?;
    let reference = quote.get("reference").as_i64().unwrap_or(1).max(1);
    let price = crate::activities::market::opening_price(false, reference, quote.get("bid").as_i64(), quote.get("ask").as_i64(), None).max(floor.max(1));
    let reply = market_call(
        "place",
        vec![
            ("account", str_value(&bot.username)),
            ("side", str_value("sell")),
            ("item", Value::Int(i64::from(item))),
            ("qty", Value::Int(qty)),
            ("price", Value::Int(price)),
            ("floor", Value::Int(floor.max(1).min(price))),
        ],
    )?;
    if reply.get("ok").as_bool() != Some(true) {
        bot.log(&format!("surplus sell refused: {}", reply.get("error").as_str().unwrap_or("?")));
        return Ok(false);
    }
    bot.log(&format!("surplus sell {} x{qty} @ {price} (floor {floor})", reply.get("order").as_i64().unwrap_or(-1)));
    Ok(true)
}

/// Cancel every open order of one item (D17: the drop follows the cancel).
pub fn cancel_for(bot: &mut Bot, item: u16) -> Result<(), ScriptError> {
    let reply = market_call("my_orders", vec![("account", str_value(&bot.username))])?;
    for entry in reply.get("orders").as_arr().cloned().unwrap_or_default() {
        if entry.get("status").as_str() != Some("open") || entry.get("item").as_i64() != Some(i64::from(item)) {
            continue;
        }
        let order = entry.get("order").as_i64().unwrap_or(-1);
        let done =
            market_call("cancel", vec![("account", str_value(&bot.username)), ("order", Value::Int(order))])?;
        if done.get("ok").as_bool() != Some(true) {
            bot.log(&format!("cleanup cancel {order} refused: {}", done.get("error").as_str().unwrap_or("?")));
        }
    }
    Ok(())
}

/// Items with at least one open sell order of this account.
pub fn open_sell_items(bot: &mut Bot) -> std::collections::HashSet<u16> {
    let mut listed = std::collections::HashSet::new();
    let Ok(reply) = market_call("my_orders", vec![("account", str_value(&bot.username))]) else { return listed };
    for entry in reply.get("orders").as_arr().cloned().unwrap_or_default() {
        if entry.get("status").as_str() != Some("open") || entry.get("side").as_str() != Some("sell") {
            continue;
        }
        if let Some(item) = entry.get("item").as_i64() {
            listed.insert(item as u16);
        }
    }
    listed
}
