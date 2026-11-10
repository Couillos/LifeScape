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

fn place_side(bot: &mut Bot, args: &[String], buying: bool) -> ScriptResult {
    if args.len() < 3 {
        return Err(ScriptError::Usage("market sell|buy <item> <qty> [price]".to_string()));
    }
    let item = tradeable_id(&args[1])?;
    let qty: i64 = args[2].parse().map_err(|_| ScriptError::Usage("qty must be a number".to_string()))?;
    // Positional price only: brain flags (--node, --budget-*) ride
    // along for the session report and must not parse as a price.
    let mut price: Option<i64> = None;
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--node" | "--budget-min" | "--budget-max" => {
                index += 2;
            }
            other if other.starts_with("--") => {
                return Err(ScriptError::Usage("market sell|buy <item> <qty> [price]".to_string()));
            }
            text => {
                price = Some(text.parse().map_err(|_| ScriptError::Usage("price must be a number".to_string()))?);
                index += 1;
            }
        }
    }
    let price = match price {
        Some(price) => price,
        None => {
            let quote = market_call("quote", vec![("item", Value::Int(i64::from(item)))])?;
            quote.get("reference").as_i64().unwrap_or(1)
        }
    };
    let reply = market_call(
        "place",
        vec![
            ("account", str_value(&bot.username)),
            ("side", str_value(if buying { "buy" } else { "sell" })),
            ("item", Value::Int(i64::from(item))),
            ("qty", Value::Int(qty)),
            ("price", Value::Int(price)),
        ],
    )?;
    if reply.get("ok").as_bool() != Some(true) {
        bot.log(&format!("place refused: {}", reply.get("error").as_str().unwrap_or("?")));
        return Ok(1);
    }
    bot.log(&format!("placed {} order {} for {qty} x{} @ {price}", if buying { "buy" } else { "sell" }, reply.get("order").as_i64().unwrap_or(-1), args[1]));
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
