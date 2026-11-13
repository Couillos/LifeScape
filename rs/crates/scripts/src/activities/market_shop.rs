//! Market shop buys: NPC shop legs of the planner's `shop:buy` method.
//! Split from `market.rs` so every activity stays under the plafond.
//! Uses the shared `buy_stock` helper (nearest stocking shop, travel,
//! keeper trade): the brain files the obtain task, this only executes.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::obj_id;
use crate::{ScriptError, ScriptResult};

/// One `market shopbuy` order as parsed: the item, its quantity, the
/// planner's shop key (`--shop`, `gamedata::shops` name) and keeper site
/// (`--site`), and whether the toll gate may be paid (`toll`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopOrder {
    pub item: String,
    pub qty: u32,
    pub shop: Option<String>,
    pub site: Option<(i32, i32, i32)>,
    pub toll: bool,
    /// Uniform a guild keeper's door checks (`--wear`, worn before the walk).
    pub wear: Vec<String>,
}

/// Parse `market shopbuy <item> <qty> [--shop KEY] [--site x,z,level]
/// [toll] [--node ID] [--budget-* N]` (no world access).
pub fn parse_shopbuy(args: &[String]) -> Result<ShopOrder, ScriptError> {
    const USAGE: &str = "market shopbuy <item> <qty> [--shop KEY] [--site x,z,level] [--wear ITEM]... [toll]";
    let usage = || ScriptError::Usage(USAGE.to_string());
    let mut positionals: Vec<String> = Vec::new();
    let mut order = ShopOrder { item: String::new(), qty: 1, shop: None, site: None, toll: false, wear: Vec::new() };
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--budget-max" | "--budget-min" | "--node" => index += 2,
            "toll" => {
                order.toll = true;
                index += 1;
            }
            "--shop" => {
                order.shop = Some(args.get(index + 1).filter(|key| !key.is_empty() && !key.starts_with("--")).ok_or_else(usage)?.clone());
                index += 2;
            }
            "--wear" => {
                order.wear.push(args.get(index + 1).filter(|item| !item.is_empty() && !item.starts_with("--")).ok_or_else(usage)?.clone());
                index += 2;
            }
            "--site" => {
                let raw = args.get(index + 1).ok_or_else(usage)?;
                let parts: Vec<i32> = raw.split(',').map(|part| part.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| usage())?;
                let [x, z, level] = parts[..] else { return Err(usage()) };
                order.site = Some((x, z, level));
                index += 2;
            }
            other if other.starts_with("--") => return Err(usage()),
            _ => {
                positionals.push(args[index].clone());
                index += 1;
            }
        }
    }
    order.item = positionals.first().cloned().ok_or_else(usage)?;
    if let Some(raw) = positionals.get(1) {
        order.qty = raw.parse().map_err(|_| ScriptError::Usage("qty must be a number".to_string()))?;
    }
    Ok(order)
}

/// `market shopbuy <item> <qty> [--shop KEY] [--site x,z,level] [toll]`:
/// buy at the planner's shop (`--shop`), from its keeper nearest the
/// planner's site, never another shop. Without `--shop` (manual runs)
/// the nearest stocking shop sells. The exit is 0 when the full quantity
/// landed, 1 otherwise (the brain closes obtain nodes on holdings, never
/// on this code alone).
pub fn run_shopbuy(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let ShopOrder { item, qty, shop, site, toll, wear } = parse_shopbuy(args)?;
    if toll {
        bot.allow_paid = true;
    }
    let entry = lifescape_gamedata::objs_by_name(&item)
        .into_iter()
        .next()
        .ok_or_else(|| ScriptError::Usage(format!("unknown item {item:?}")))?;
    if entry.members == Some(true) {
        return Err(ScriptError::Usage(format!("{item:?} is not F2P (L1 refusal)")));
    }
    // Wallet is pack-first, bank-backed: withdraw the shortfall so a
    // funded bot is never stopped by pocket change. The shop itself
    // refuses true broke (0 bought, loud, no silent spend). A fresh
    // session has not seen its bank yet (live 2026-11-12: harry_page
    // went to the shop "carrying 0 coins" with 25 banked, three times):
    // the bank is opened to look whenever the pack falls short.
    let coins = obj_id(bot, "coins")?;
    let price_each = entry.price.max(1) as u32;
    let need = qty.max(1) as u32 * price_each;
    let mut held = bot.inventory().get(&coins).copied().unwrap_or(0);
    if held < need {
        let bank = crate::kit::bank::nearest_bank(bot)?;
        if bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
            bot.pump(2)?;
            let banked = bot.bank_items().get(&coins).copied().unwrap_or(0);
            if banked > 0 {
                let _ = bot.bank_withdraw(coins, (need - held).min(banked), 300);
            }
            let _ = bot.close_modal();
            bot.pump(3)?;
        }
        held = bot.inventory().get(&coins).copied().unwrap_or(0);
    }
    bot.log(&format!("shopbuy {item} x{qty}: carrying {held} coins"));
    if let Some((x, z, _)) = site {
        bot.assigned_site = site;
        crate::kit::uniform::wear_uniform(bot, &wear)?;
        if !crate::kit::site::at_site(bot) && !crate::skill_walk::walk_checked(bot, x, z, crate::kit::site::SITE_WALK_TICKS)? {
            return Err(ScriptError::Failed(format!("shopbuy {item}: shop site ({x},{z}) not reached from {:?}", bot.position())));
        }
    }
    let bought = crate::kit::shop::buy_stock_at(bot, &item, qty, shop.as_deref())?;
    let held = obj_id(bot, &item).map(|id| bot.inventory().get(&id).copied().unwrap_or(0)).unwrap_or(0);
    bot.log(&format!("shopbuy {item} x{qty}: bought {bought}, holding {held}"));
    if bought == 0 && held < qty {
        let from = shop.as_deref().map_or_else(|| "no reachable shop".to_string(), |key| format!("shop {key}"));
        return Err(ScriptError::Failed(format!("shopbuy {item}: {from} sold none at {:?}", bot.position())));
    }
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
