//! NPC shop sale leg of `market cleanup` (plan marché Q3, user decision
//! 2026-11-16): the brain names the shop, its keeper site and the lots
//! (`--sale-shop KEY --sale-site x,z,level --shopsell ITEM:QTY:MIN`...),
//! this only executes: withdraw the lots (banknotes when the object has
//! one), walk to the keeper, sell unit by unit while the shop pays at
//! least MIN per unit (what the market pays + 1: never a sale at a loss
//! against the market, `kit::shop::sell_units`). The coins are tagged
//! `shop_sold` by the coin ledger.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::obj_id;
use crate::ScriptError;

/// One lot of a shop sale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleLot {
    pub item: String,
    pub qty: u32,
    pub min_unit: i64,
}

/// The shop trip the brain filed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleOrder {
    pub shop: String,
    pub site: (i32, i32, i32),
    pub lots: Vec<SaleLot>,
}

/// Parses one `ITEM:QTY:MIN` lot (no world access).
pub fn parse_lot(raw: &str) -> Result<SaleLot, ScriptError> {
    let usage = || ScriptError::Usage(format!("--shopsell wants ITEM:QTY:MIN, got {raw:?}"));
    let mut parts = raw.split(':');
    let (Some(item), Some(qty), Some(min_unit), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return Err(usage()) };
    if item.is_empty() {
        return Err(usage());
    }
    Ok(SaleLot { item: item.to_string(), qty: qty.parse().map_err(|_| usage())?, min_unit: min_unit.parse().map_err(|_| usage())? })
}

/// Parses `x,z,level`.
pub fn parse_site(raw: &str) -> Result<(i32, i32, i32), ScriptError> {
    let parts: Vec<i32> = raw.split(',').map(|part| part.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| ScriptError::Usage(format!("--sale-site wants x,z,level, got {raw:?}")))?;
    let [x, z, level] = parts[..] else { return Err(ScriptError::Usage(format!("--sale-site wants x,z,level, got {raw:?}"))) };
    Ok((x, z, level))
}

/// Splits the shop-sale flags (`--sale-shop KEY --sale-site x,z,level
/// --shopsell ITEM:QTY:MIN`...) off a `market cleanup` command line:
/// the other arguments, and the trip when the three are present (one
/// without the others is a typed `Usage`).
pub fn split_sale_args(args: &[String]) -> Result<(Vec<String>, Option<SaleOrder>), ScriptError> {
    let usage = || ScriptError::Usage("a shop sale needs --sale-shop KEY, --sale-site x,z,level and at least one --shopsell ITEM:QTY:MIN".to_string());
    let (mut rest, mut shop, mut site, mut lots) = (Vec::new(), None, None, Vec::new());
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1).filter(|value| !value.is_empty());
        match args[index].as_str() {
            "--sale-shop" => shop = Some(value.ok_or_else(usage)?.clone()),
            "--sale-site" => site = Some(parse_site(value.ok_or_else(usage)?)?),
            "--shopsell" => lots.push(parse_lot(value.ok_or_else(usage)?)?),
            other => {
                rest.push(other.to_string());
                index += 1;
                continue;
            }
        }
        index += 2;
    }
    let sale = match (shop, site, lots.is_empty()) {
        (None, None, true) => None,
        (Some(shop), Some(site), false) => Some(SaleOrder { shop, site, lots }),
        _ => return Err(usage()),
    };
    Ok((rest, sale))
}

/// Runs the trip (none: false at once), then reports whether the bank
/// has room again (the janitor pass is not needed).
pub fn sale_then_room(bot: &mut Bot, order: Option<&SaleOrder>) -> Result<bool, ScriptError> {
    let Some(order) = order else { return Ok(false) };
    let sold = run_shop_sale(bot, order)?;
    bot.log(&format!("cleanup: {sold} units sold to {}", order.shop));
    let bank = crate::kit::bank::nearest_bank(bot)?;
    if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
        return Err(ScriptError::Blocked { missing: vec!["bank".to_string()] });
    }
    let room = crate::kit::janitor::known_bank_types(bot) < lifescape_planner::space::BANK_TYPE_LIMIT;
    if room {
        let _ = bot.close_modal();
    }
    Ok(room)
}

/// The keeper of `shop` standing at `site` (an F2P opener of the shop).
fn keeper_at(shop: &str, site: (i32, i32, i32)) -> Option<String> {
    let access = lifescape_gamedata::f2p::f2p_shop_access(shop)?;
    access.npcs.iter().filter_map(|npc| lifescape_gamedata::npc_by_id(*npc)).find(|npc| npc.spawns.iter().any(|spawn| (spawn.x, spawn.z, spawn.level) == (i64::from(site.0), i64::from(site.1), i64::from(site.2)))).and_then(|npc| npc.name).map(str::to_string)
}

/// Runs the trip; returns the units sold. Nothing carried or banked of a
/// lot skips it; an unreachable keeper fails typed.
pub fn run_shop_sale(bot: &mut Bot, order: &SaleOrder) -> Result<u32, ScriptError> {
    let keeper = keeper_at(&order.shop, order.site).ok_or_else(|| ScriptError::Usage(format!("no F2P keeper of {} at {:?}", order.shop, order.site)))?;
    // Withdraw every lot (banknotes for non-stackables: one slot each).
    let bank = crate::kit::bank::nearest_bank(bot)?;
    if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
        return Err(ScriptError::Blocked { missing: vec!["bank".to_string()] });
    }
    bot.pump(2)?;
    let mut forms: Vec<(SaleLot, u16)> = Vec::new();
    for lot in &order.lots {
        let base = obj_id(bot, &lot.item)?;
        let banked = bot.bank_items().get(&base).copied().unwrap_or(0);
        let take = lot.qty.min(banked);
        let note = crate::market_client::trade_id(base);
        if take > 0 {
            let outcome = if note != base { bot.bank_withdraw_noted(base, note, take, 300) } else { bot.bank_withdraw(base, take, 300) };
            if let Err(error) = outcome {
                bot.log(&format!("shopsell {}: withdraw failed ({error})", lot.item));
            }
        }
        let form = if crate::kit::pack::pack_total(bot, note) > 0 { note } else { base };
        let carried = crate::kit::pack::pack_total(bot, form).min(lot.qty);
        if carried > 0 {
            forms.push((SaleLot { qty: carried, ..lot.clone() }, form));
        }
    }
    let _ = bot.close_modal();
    bot.pump(3)?;
    if forms.is_empty() {
        bot.log("shopsell: nothing to sell (lots gone since the plan)");
        return Ok(0);
    }
    let (x, z, level) = order.site;
    bot.assigned_site = Some(order.site);
    if !crate::kit::site::at_site(bot) && !crate::skill_walk::walk_checked_at(bot, x, z, level, crate::kit::site::SITE_WALK_TICKS)? {
        return Err(ScriptError::Failed(format!("shopsell: {} site ({x},{z}) not reached from {:?}", order.shop, bot.position())));
    }
    if !crate::kit::shop::trade_to_keeper(bot, &keeper, order.site)? {
        return Err(ScriptError::Failed(format!("shopsell: trade with {keeper} failed at {:?}", bot.position())));
    }
    let mut total = 0u32;
    for (lot, form) in &forms {
        let (sold, coins) = crate::kit::shop::sell_units(bot, *form, &lot.item, &order.shop, lot.qty, lot.min_unit)?;
        bot.log(&format!("shopsell {}: sold {sold}/{} to {} for {coins} coins (at least {} each)", lot.item, lot.qty, order.shop, lot.min_unit));
        total += sold;
    }
    bot.close_modal()?;
    bot.pump(3)?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lots_and_sites_parse_strictly() {
        assert_eq!(parse_lot("iron_dagger:40:1").ok(), Some(SaleLot { item: "iron_dagger".to_string(), qty: 40, min_unit: 1 }));
        assert!(parse_lot("iron_dagger:40").is_err());
        assert!(parse_lot("iron_dagger:x:1").is_err());
        assert!(parse_lot(":4:1").is_err());
        assert!(parse_lot("a:1:2:3").is_err());
        assert_eq!(parse_site("3212,3246,0").ok(), Some((3212, 3246, 0)));
        assert!(parse_site("3212,3246").is_err());
    }

    #[test]
    fn sale_flags_split_off_the_janitor_line() {
        let line: Vec<String> = ["cleanup", "--keep", "coins", "--sale-shop", "generalshop1", "--sale-site", "3212,3246,0", "--shopsell", "iron_dagger:40:1"].iter().map(|arg| arg.to_string()).collect();
        let (rest, sale) = split_sale_args(&line).expect("parses");
        assert_eq!(rest, vec!["cleanup", "--keep", "coins"]);
        let sale = sale.expect("a trip");
        assert_eq!((sale.shop.as_str(), sale.site, sale.lots.len()), ("generalshop1", (3212, 3246, 0), 1));
        assert!(split_sale_args(&line[..5]).is_err(), "a shop without a site or a lot is a Usage");
        assert_eq!(split_sale_args(&line[..3]).expect("plain").1, None);
    }
}
