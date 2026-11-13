//! Market activity: place orders, deliver physical handovers on the
//! Varrock square. Stateless: every argument arrives in the pose, the
//! ledger and the carnet live daemon-side.

#![forbid(unsafe_code)]

use lifescape_json::{str_value, Value};
use lifescape_sdk::Bot;

use crate::market_client::market_call;
use crate::{ScriptError, ScriptResult};

/// `market sell|buy <item> <qty> [price]` (price defaults to reference),
pub fn run_market(bot: &mut Bot, args: &[String]) -> ScriptResult {
    match args.first().map(String::as_str) {
        Some("sell") => place_side(bot, args, false),
        Some("buy") => place_side(bot, args, true),
        Some("flip") => super::market_flip::run_flip(bot, args),
        Some("cleanup") => super::market_cleanup::run_cleanup(bot, args),
        Some("shopbuy") => super::market_shop::run_shopbuy(bot, args),
        Some("deliver") => super::market_deliver::run_deliver(bot, args),
        _ => Err(ScriptError::Usage("market <sell|buy|flip|shopbuy|deliver|cleanup> ...".to_string())),
    }
}

pub(super) fn tradeable_id(name: &str) -> Result<u16, ScriptError> {
    let entry = lifescape_gamedata::objs_by_name(name).into_iter().next().ok_or_else(|| ScriptError::Usage(format!("unknown item {name:?}")))?;
    if !entry.tradeable || entry.members != Some(false) {
        return Err(ScriptError::Usage(format!("{name:?} is not F2P tradeable (L1 refusal)")));
    }
    Ok(entry.obj_id as u16)
}

/// Opening price of an unpriced order (PRD3 lot 2). A buyer takes the
/// best ask when its ceiling covers it, else opens at the reference
/// (never above the ceiling); a seller asks the reference, or the best
/// bid when a buyer already pays more (2026-11-13: no margin on top, the
/// daemon walks a listing up while it sells). Mirrors
/// `lifescape_market::{opening_bid, opening_ask}` (no market dependency
/// in the scripts crate).
pub(crate) fn opening_price(buying: bool, reference: i64, bid: Option<i64>, ask: Option<i64>, ceiling: Option<i64>) -> i64 {
    let reference = reference.max(1);
    if buying {
        let cap = ceiling.unwrap_or(reference).max(1);
        match ask {
            Some(best) if best <= cap => best,
            _ => reference.min(cap),
        }
    } else {
        reference.max(bid.unwrap_or(0))
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
        Some("flip") => super::market_flip::parse_flip(args).map(drop),
        Some("cleanup") => super::market_cleanup::parse_args(args).map(drop),
        Some("shopbuy") => super::market_shop::parse_shopbuy(args).map(drop),
        Some("deliver") => crate::market_handover::parse_deliver_args(args).map(drop),
        _ => Err(ScriptError::Usage("market <sell|buy|flip|shopbuy|deliver|cleanup> ...".to_string())),
    }
}

fn place_side(bot: &mut Bot, args: &[String], buying: bool) -> ScriptResult {
    let SideOrder { item, qty, price, ceiling, mut floor } = parse_side(args)?;
    let quote = market_call("quote", vec![("item", Value::Int(i64::from(item)))])?;
    let reference = quote.get("reference").as_i64().unwrap_or(1).max(1);
    let price = price.unwrap_or_else(|| opening_price(buying, reference, quote.get("bid").as_i64(), quote.get("ask").as_i64(), ceiling));
    // Sellers walk down to a floor: half a traded reference by default,
    // 1 coin for a never-traded object (the buyers set its price).
    if !buying && floor.is_none() {
        floor = Some(if quote.get("traded").as_bool() == Some(true) { (reference / 2).max(1) } else { 1 });
    }
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
    let reply = market_call("place", params)?;
    if reply.get("ok").as_bool() != Some(true) {
        bot.log(&format!("place refused: {}", reply.get("error").as_str().unwrap_or("?")));
        return Ok(1);
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
    fn opening_prices_follow_the_book() {
        use super::opening_price;
        assert_eq!(opening_price(true, 1, None, None, Some(20)), 1, "untraded: bid opens at 1");
        assert_eq!(opening_price(true, 1, None, Some(12), Some(20)), 12, "covered ask: take it");
        assert_eq!(opening_price(true, 10, None, Some(30), Some(20)), 10, "ask above the ceiling: rest at the reference");
        assert_eq!(opening_price(true, 50, None, None, Some(20)), 20, "never above the ceiling");
        assert_eq!(opening_price(false, 100, None, None, None), 100, "ask at the reference");
        assert_eq!(opening_price(false, 100, Some(130), None, None), 130, "a richer bid wins");
        assert_eq!(opening_price(false, 1, None, None, None), 1, "untraded seller asks 1");
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
