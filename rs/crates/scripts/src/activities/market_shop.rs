//! Market shop buys: NPC shop legs of the planner's `shop:buy` method.
//! Split from `market.rs` so every activity stays under the plafond.
//! Uses the shared `buy_stock` helper (nearest stocking shop, travel,
//! keeper trade): the brain files the obtain task, this only executes.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::obj_id;
use crate::{ScriptError, ScriptResult};

/// `market shopbuy <item> <qty> [--budget-max N] [--node ID]`: buy from
/// the nearest F2P shop stocking the item. `--node`/`--budget-max` ride
/// along for the session report like every brain order; the exit is 0
/// when the full quantity landed, 1 otherwise (the brain closes obtain
/// nodes on holdings, never on this code alone).
pub fn run_shopbuy(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let mut item: Option<String> = None;
    let mut qty: u32 = 1;
    let mut index = 1;
    let mut positionals: Vec<String> = Vec::new();
    while index < args.len() {
        match args[index].as_str() {
            "--budget-max" | "--node" => {
                index += 2;
            }
            other if other.starts_with("--") => {
                return Err(ScriptError::Usage("market shopbuy <item> <qty>".to_string()));
            }
            _ => {
                positionals.push(args[index].clone());
                index += 1;
            }
        }
    }
    if let Some(name) = positionals.first() {
        item = Some(name.clone());
    }
    if let Some(raw) = positionals.get(1) {
        qty = raw.parse().map_err(|_| ScriptError::Usage("qty must be a number".to_string()))?;
    }
    let item = item.ok_or_else(|| ScriptError::Usage("market shopbuy <item> <qty>".to_string()))?;
    let entry = lifescape_gamedata::objs_by_name(&item)
        .into_iter()
        .next()
        .ok_or_else(|| ScriptError::Usage(format!("unknown item {item:?}")))?;
    if entry.members == Some(true) {
        return Err(ScriptError::Usage(format!("{item:?} is not F2P (L1 refusal)")));
    }
    // Wallet is pack-first, bank-backed: withdraw the shortfall so a
    // funded bot is never stopped by pocket change. The shop itself
    // refuses true broke (0 bought, loud, no silent spend).
    let coins = obj_id(bot, "coins")?;
    let price_each = entry.price.max(1) as u32;
    let need = qty.max(1) as u32 * price_each;
    let mut held = bot.inventory().get(&coins).copied().unwrap_or(0);
    if held < need {
        let banked = bot.bank_items().get(&coins).copied().unwrap_or(0);
        if banked > 0 {
            let bank = crate::kit::bank::nearest_bank(bot)?;
            if bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
                let _ = bot.bank_withdraw(coins, (need - held).min(banked), 300);
                let _ = bot.close_modal();
                bot.pump(3)?;
            }
            held = bot.inventory().get(&coins).copied().unwrap_or(0);
        }
    }
    bot.log(&format!("shopbuy {item} x{qty}: carrying {held} coins"));
    let bought = crate::kit::shop::buy_stock(bot, &item, qty)?;
    let held = obj_id(bot, &item).map(|id| bot.inventory().get(&id).copied().unwrap_or(0)).unwrap_or(0);
    bot.log(&format!("shopbuy {item} x{qty}: bought {bought}, holding {held}"));
    Ok(if held >= qty { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::run_shopbuy;

    #[test]
    fn shopbuy_rejects_junk() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        assert!(run_shopbuy(&mut bot, &["shopbuy".to_string()]).is_err(), "item required");
        assert!(run_shopbuy(&mut bot, &["shopbuy".to_string(), "net".to_string(), "x".to_string()]).is_err(), "qty numeric");
        assert!(run_shopbuy(&mut bot, &["shopbuy".to_string(), "nope_not_real".to_string(), "1".to_string()]).is_err(), "unknown item fails loud");
    }
}
