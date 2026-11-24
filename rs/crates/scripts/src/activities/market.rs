//! Market activity: place orders, deliver physical handovers on the
//! Varrock square. Stateless: every argument arrives in the pose, the
//! ledger and the carnet live daemon-side.

#![forbid(unsafe_code)]

use crate::market_ops::MarketOp;
use lifescape_json::{str_value, Value};
use lifescape_sdk::Bot;

use crate::market_client::market_call;
use crate::{ScriptError, ScriptResult};

/// `market sell|buy <item> <qty> [price]` (price defaults to reference),
pub fn run_market(bot: &mut Bot, args: &[String]) -> ScriptResult {
    match args.first().map(String::as_str) {
        Some("sell") => place_side(bot, args, false),
        Some("buy") => place_side(bot, args, true),
        Some("make") => super::market_make::run_make(bot, args),
        Some("cleanup") => super::market_cleanup::run_cleanup(bot, args),
        Some("shopbuy") => super::market_shop::run_shopbuy(bot, args),
        Some("deliver") => super::market_deliver::run_deliver(bot, args),
        _ => Err(ScriptError::Usage("market <sell|buy|make|shopbuy|deliver|cleanup> ...".to_string())),
    }
}

pub(super) fn tradeable_id(name: &str) -> Result<u16, ScriptError> {
    let entry = lifescape_gamedata::objs_by_name(name).into_iter().next().ok_or_else(|| ScriptError::Usage(format!("unknown item {name:?}")))?;
    if !entry.tradeable || entry.members != Some(false) {
        return Err(ScriptError::Usage(format!("{name:?} is not F2P tradeable (L1 refusal)")));
    }
    Ok(entry.obj_id as u16)
}

/// Opening price of an unpriced buy (PRD3 lot 2): the best ask when the
/// ceiling covers it, else the reference, never above the ceiling.
/// Mirrors `lifescape_market::opening_bid` (no market dependency in the
/// scripts crate). Sells never price here: the daemon's `seller_quote`
/// prices them from the seller's own reserve (2026-11-14).
pub(crate) fn opening_bid_price(reference: i64, ask: Option<i64>, ceiling: Option<i64>) -> i64 {
    let reference = reference.max(1);
    let cap = ceiling.unwrap_or(reference).max(1);
    match ask {
        Some(best) if best <= cap => best,
        _ => reference.min(cap),
    }
}

/// One `market sell|buy` order as parsed: item, quantity, optional
/// price, buyer ceiling, seller floor.
#[derive(Debug)]
pub struct SideOrder {
    pub item: u16,
    pub qty: i64,
    pub price: Option<i64>,
    pub ceiling: Option<i64>,
    pub floor: Option<i64>,
}

/// Parse `market sell|buy <item> <qty> [price] [--ceiling N] [--floor N]`
/// (no world access). Brain flags (`--node`, `--budget-*`) ride along for
/// the session report and never parse as a price.
pub fn parse_side(args: &[String]) -> Result<SideOrder, ScriptError> {
    const USAGE: &str = "market sell|buy <item> <qty> [price] [--ceiling N] [--floor N]";
    if args.len() < 3 {
        return Err(ScriptError::Usage(USAGE.to_string()));
    }
    let item = tradeable_id(&args[1])?;
    let qty: i64 = args[2].parse().map_err(|_| ScriptError::Usage("qty must be a number".to_string()))?;
    let mut order = SideOrder { item, qty, price: None, ceiling: None, floor: None };
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--node" | "--budget-min" | "--budget-max" => {
                index += 2;
            }
            "--ceiling" | "--floor" => {
                let value: i64 = args
                    .get(index + 1)
                    .and_then(|text| text.parse().ok())
                    .ok_or_else(|| ScriptError::Usage(USAGE.to_string()))?;
                if args[index] == "--ceiling" {
                    order.ceiling = Some(value.max(1));
                } else {
                    order.floor = Some(value.max(1));
                }
                index += 2;
            }
            other if other.starts_with("--") => {
                return Err(ScriptError::Usage(USAGE.to_string()));
            }
            text => {
                order.price = Some(text.parse().map_err(|_| ScriptError::Usage("price must be a number".to_string()))?);
                index += 1;
            }
        }
    }
    Ok(order)
}

/// Contract check of a `market` launch (no world access): the
/// subcommand's own parser.
pub fn parse_order(args: &[String]) -> Result<(), ScriptError> {
    match args.first().map(String::as_str) {
        Some("sell") | Some("buy") => parse_side(args).map(drop),
        Some("make") => super::market_make::parse_make(args).map(drop),
        Some("cleanup") => super::market_cleanup::parse_args(args).map(drop),
        Some("shopbuy") => super::market_shop::parse_shopbuy(args).map(drop),
        Some("deliver") => crate::market_handover::parse_deliver_args(args).map(drop),
        _ => Err(ScriptError::Usage("market <sell|buy|make|shopbuy|deliver|cleanup> ...".to_string())),
    }
}

/// Typed refusal of a `place` (P16): the market's own reason, side and
/// object named, never « run returned 1 ».
pub(crate) fn place_refused(buying: bool, item: &str, reason: &str) -> ScriptError {
    ScriptError::Failed(format!("market {} {item} refused: {reason}", if buying { "buy" } else { "sell" }))
}

fn place_side(bot: &mut Bot, args: &[String], buying: bool) -> ScriptResult {
    let SideOrder { item, mut qty, price, ceiling, mut floor } = parse_side(args)?;
    let price = if buying {
        let quote = market_call(MarketOp::Quote, vec![("item", Value::Int(i64::from(item)))])?;
        let reference = quote.get("reference").as_i64().unwrap_or(1).max(1);
        price.unwrap_or_else(|| opening_bid_price(reference, quote.get("ask").as_i64(), ceiling))
    } else {
        // Sellers price from their own reserve (daemon `seller_quote`):
        // a demand-sized lot, never under the reserve, never a 1-coin
        // fallback; no known own cost = hold.
        // A hold is a typed block, never a bare exit code (plan P16, rule
        // 6: 71 « run returned 1 » the brain could not class).
        let Some(quoted) = seller_quote(bot, item, qty)? else {
            return Err(ScriptError::Failed(format!("market sell {}: held (no own cost known or nothing free to list)", args.get(1).map(String::as_str).unwrap_or("?"))));
        };
        qty = quoted.qty;
        let reserve = floor.unwrap_or(quoted.floor).max(quoted.floor);
        floor = Some(reserve);
        price.unwrap_or(quoted.price).max(reserve)
    };
    let mut params = vec![
        ("account", str_value(&bot.username)),
        ("side", str_value(if buying { "buy" } else { "sell" })),
        ("item", Value::Int(i64::from(item))),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ];
    if let Some(cap) = ceiling.filter(|_| buying) {
        params.push(("ceiling", Value::Int(cap.max(price))));
    }
    if let Some(reserve) = floor.filter(|_| !buying) {
        params.push(("floor", Value::Int(reserve.min(price))));
    }
    let reply = market_call(MarketOp::Place, params)?;
    if reply.get("ok").as_bool() != Some(true) {
        let reason = reply.get("error").as_str().unwrap_or("?").to_string();
        bot.log(&format!("place refused: {reason}"));
        return Err(place_refused(buying, args.get(1).map(String::as_str).unwrap_or("?"), &reason));
    }
    bot.log(&format!(
        "placed {} order {} for {qty} x{} @ {price} (ceiling {:?}, floor {:?})",
        if buying { "buy" } else { "sell" },
        reply.get("order").as_i64().unwrap_or(-1),
        args[1],
        ceiling,
        floor
    ));
    Ok(0)
}

/// A seller's own listing, as the daemon prices it (`seller_quote`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SellerQuote {
    pub qty: i64,
    pub price: i64,
    pub floor: i64,
}

impl SellerQuote {
    /// `Ok(None)` = hold (no own cost known, nothing free, band).
    pub(crate) fn from_reply(reply: &Value) -> Result<Option<SellerQuote>, String> {
        if reply.get("ok").as_bool() != Some(true) {
            return Err(reply.get("error").as_str().unwrap_or("seller_quote refused").to_string());
        }
        if reply.get("hold").as_str().is_some() {
            return Ok(None);
        }
        match (reply.get("qty").as_i64(), reply.get("price").as_i64(), reply.get("floor").as_i64()) {
            (Some(qty), Some(price), Some(floor)) => Ok(Some(SellerQuote { qty, price, floor })),
            _ => Err("seller_quote without qty/price/floor".to_string()),
        }
    }
}

/// The daemon's quote for selling `qty` of `item` (the bot's own reserve
/// and demand lot); `None` = hold, logged.
pub(crate) fn seller_quote(bot: &mut Bot, item: u16, qty: i64) -> Result<Option<SellerQuote>, ScriptError> {
    let reply = market_call(MarketOp::SellerQuote, vec![("account", str_value(&bot.username)), ("item", Value::Int(i64::from(item))), ("qty", Value::Int(qty))])?;
    match SellerQuote::from_reply(&reply) {
        Ok(Some(quote)) => Ok(Some(quote)),
        Ok(None) => {
            bot.log(&format!("sell {item}: holding ({})", reply.get("hold").as_str().unwrap_or("?")));
            Ok(None)
        }
        Err(error) => {
            bot.log(&format!("sell {item}: no quote ({error})"));
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::place_side;
    use crate::market_client::buyer_want;

    /// Brain flags ride along but never parse as the price: a missing
    /// price falls back to the quote instead of failing loud.
    #[test]
    fn brain_flags_skip_the_price_slot() {
        // No market socket here: the parse must fail on junk flags, not
        // on the node id. Unknown flags fail loud before any socket use.
        let botched = ["buy".to_string(), "feather".to_string(), "10".to_string(), "--frobnicate".to_string()];
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        assert!(place_side(&mut bot, &botched, true).is_err(), "unknown flags fail loud");
    }

    #[test]
    fn opening_bids_follow_the_book() {
        use super::opening_bid_price;
        assert_eq!(opening_bid_price(1, None, Some(20)), 1, "untraded: bid opens at the reference");
        assert_eq!(opening_bid_price(1, Some(12), Some(20)), 12, "covered ask: take it");
        assert_eq!(opening_bid_price(10, Some(30), Some(20)), 10, "ask above the ceiling: rest at the reference");
        assert_eq!(opening_bid_price(50, None, Some(20)), 20, "never above the ceiling");
    }

    #[test]
    fn seller_quotes_parse_lists_and_holds() {
        use lifescape_json::{object, str_value, Value};
        let listed = object(vec![("ok", Value::Bool(true)), ("qty", Value::Int(25)), ("price", Value::Int(8)), ("floor", Value::Int(4))]);
        assert_eq!(super::SellerQuote::from_reply(&listed), Ok(Some(super::SellerQuote { qty: 25, price: 8, floor: 4 })));
        let held = object(vec![("ok", Value::Bool(true)), ("hold", str_value("no own cost known"))]);
        assert_eq!(super::SellerQuote::from_reply(&held), Ok(None), "a hold lists nothing");
        // P16: a refused place is a typed failure naming side, object and reason.
        let refused = super::place_refused(false, "iron_axe", "market rejected: seller holds 0, needs 1");
        assert_eq!(refused.to_string().contains("market sell iron_axe refused: market rejected"), true, "{refused}");
        let refused = object(vec![("ok", Value::Bool(false)), ("error", str_value("unknown item 7"))]);
        assert!(super::SellerQuote::from_reply(&refused).is_err());
    }

    #[test]
    fn buyer_takes_either_form_exact_only() {
        use std::collections::HashMap;

        // The buyer takes the carried form (base or note) at the exact
        // quantity, and nothing else: overpay, short count and coin-mix
        // all refuse.
        let base = 314u16;
        let note = 9999u16;
        let exact: HashMap<u16, u32> = [(base, 10)].into_iter().collect();
        assert_eq!(buyer_want(&exact, base, note, 10), Some(vec![(base, 10)]));
        let noted: HashMap<u16, u32> = [(note, 10)].into_iter().collect();
        assert_eq!(buyer_want(&noted, base, note, 10), Some(vec![(note, 10)]));
        let short: HashMap<u16, u32> = [(base, 9)].into_iter().collect();
        assert_eq!(buyer_want(&short, base, note, 10), None);
        let over: HashMap<u16, u32> = [(base, 10), (note, 1)].into_iter().collect();
        assert_eq!(buyer_want(&over, base, note, 10), None);
        let coins: HashMap<u16, u32> = [(base, 10), (995, 5)].into_iter().collect();
        assert_eq!(buyer_want(&coins, base, note, 10), None);
        assert_eq!(buyer_want(&HashMap::new(), base, note, 10), None);
    }
}
