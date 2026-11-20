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

/// The sale site `(x, z, level)` in the spawn order `trade_to_keeper`
/// reads, `(level, x, z)`. Handed over raw, the keeper walk asked for
/// (z, level) as a tile: "walk to spawn (3511,0)" (live 2026-11-20).
fn keeper_spawn(site: (i32, i32, i32)) -> (i32, i32, i32) {
    let (x, z, level) = site;
    (level, x, z)
}

/// The keeper of `shop` standing at `site` (an F2P opener of the shop).
fn keeper_at(shop: &str, site: (i32, i32, i32)) -> Option<String> {
    let access = lifescape_gamedata::f2p::f2p_shop_access(shop)?;
    access.npcs.iter().filter_map(|npc| lifescape_gamedata::npc_by_id(*npc)).find(|npc| npc.spawns.iter().any(|spawn| (spawn.x, spawn.z, spawn.level) == (i64::from(site.0), i64::from(site.1), i64::from(site.2)))).and_then(|npc| npc.name).map(str::to_string)
}

/// Takes every lot into the pack (banknotes for non-stackables: one slot
/// each), only the units not carried yet; returns the lots carried with
/// their form (empty: nothing left to sell).
fn withdraw_lots(bot: &mut Bot, lots: &[SaleLot]) -> Result<Vec<(SaleLot, u16)>, ScriptError> {
    let mut wanted: Vec<(&SaleLot, u16, u16, u32)> = Vec::new();
    for lot in lots {
        let base = obj_id(bot, &lot.item)?;
        let note = crate::market_client::trade_id(base);
        let carried = crate::kit::pack::pack_total(bot, base) + if note != base { crate::kit::pack::pack_total(bot, note) } else { 0 };
        wanted.push((lot, base, note, lot.qty.saturating_sub(carried)));
    }
    if wanted.iter().any(|(_, _, _, missing)| *missing > 0) {
        let bank = crate::kit::bank::nearest_bank(bot)?;
        if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
            return Err(ScriptError::Blocked { missing: vec!["bank".to_string()] });
        }
        bot.pump(2)?;
        for (lot, base, note, missing) in &wanted {
            let banked = bot.bank_items().get(base).copied().unwrap_or(0);
            let take = (*missing).min(banked);
            if take > 0 {
                let outcome = if note != base { bot.bank_withdraw_noted(*base, *note, take, 300) } else { bot.bank_withdraw(*base, take, 300) };
                if let Err(error) = outcome {
                    bot.log(&format!("shopsell {}: withdraw failed ({error})", lot.item));
                }
            }
        }
        let _ = bot.close_modal();
        bot.pump(3)?;
    }
    let mut forms: Vec<(SaleLot, u16)> = Vec::new();
    for (lot, base, note, _) in wanted {
        let form = if crate::kit::pack::pack_total(bot, note) > 0 { note } else { base };
        let carried = crate::kit::pack::pack_total(bot, form).min(lot.qty);
        if carried > 0 {
            forms.push((SaleLot { qty: carried, ..lot.clone() }, form));
        }
    }
    Ok(forms)
}

/// Runs the trip; returns the units sold. Nothing carried or banked of a
/// lot skips it; an unreachable keeper fails typed. The keeper trade
/// runs on the keeper's site with the lots carried (`kit::site::pass_leg`):
/// a market pause in between (walk, trade wait) sends the sale back to
/// the bank and the site, at most `PASS_PAUSE_RESTARTS` times.
pub fn run_shop_sale(bot: &mut Bot, order: &SaleOrder) -> Result<u32, ScriptError> {
    use crate::kit::site::{pass_leg, paused_since, PassLeg, PASS_PAUSE_RESTARTS};
    let keeper = keeper_at(&order.shop, order.site).ok_or_else(|| ScriptError::Usage(format!("no F2P keeper of {} at {:?}", order.shop, order.site)))?;
    let (x, z, level) = order.site;
    bot.assigned_site = Some(order.site);
    let mut pauses_at_withdraw: Option<u64> = None;
    let mut restarts = 0u32;
    let mut walked = false;
    let mut forms: Vec<(SaleLot, u16)> = Vec::new();
    // Units still to sell per lot: a pause in the sale itself resumes
    // with what is left (lot 8 review).
    let mut remaining: Vec<SaleLot> = order.lots.clone();
    let mut total = 0u32;
    'trip: loop {
        let here = (bot.world.x, bot.world.z, bot.world.level);
        match pass_leg(pauses_at_withdraw, bot.pauses_done, walked, here, Some(order.site)) {
            PassLeg::Load => {
                if pauses_at_withdraw.is_some() {
                    restarts += 1;
                    if restarts > PASS_PAUSE_RESTARTS {
                        return Err(ScriptError::Failed(format!("shopsell: {PASS_PAUSE_RESTARTS} market pauses already broke the trip to {}, at {:?}", order.shop, bot.position())));
                    }
                    bot.log(&format!("shopsell: a market pause left us at {:?}, back to the bank then {} ({x},{z})", bot.position(), order.shop));
                }
                pauses_at_withdraw = Some(bot.pauses_done);
                walked = false;
                forms = withdraw_lots(bot, &remaining)?;
                if forms.is_empty() {
                    bot.log("shopsell: nothing to sell (lots gone since the plan)");
                    return Ok(total);
                }
            }
            PassLeg::Walk => {
                if !crate::skill_walk::walk_checked_at(bot, x, z, level, crate::kit::site::SITE_WALK_TICKS)? {
                    return Err(ScriptError::Failed(format!("shopsell: {} site ({x},{z}) not reached from {:?}", order.shop, bot.position())));
                }
                walked = true;
            }
            PassLeg::Act => {
                let opened = crate::kit::shop::trade_to_keeper(bot, &keeper, keeper_spawn(order.site))?;
                if pauses_at_withdraw.is_some_and(|mark| paused_since(mark, bot.pauses_done)) {
                    // A pause during the trade wait banked the lots: the
                    // next leg takes them again (a shop open since waits).
                    if opened {
                        bot.close_modal()?;
                    }
                    continue;
                }
                if !opened {
                    return Err(ScriptError::Failed(format!("shopsell: trade with {keeper} failed at {:?}", bot.position())));
                }
                for (lot, form) in &forms {
                    let (sold, coins) = crate::kit::shop::sell_units(bot, *form, &lot.item, &order.shop, lot.qty, lot.min_unit)?;
                    bot.log(&format!("shopsell {}: sold {sold}/{} to {} for {coins} coins (at least {} each)", lot.item, lot.qty, order.shop, lot.min_unit));
                    total += sold;
                    if let Some(left) = remaining.iter_mut().find(|left| left.item == lot.item) {
                        left.qty = left.qty.saturating_sub(sold);
                    }
                    if pauses_at_withdraw.is_some_and(|mark| paused_since(mark, bot.pauses_done)) {
                        // A pause inside the sale banked the rest: resume.
                        remaining.retain(|left| left.qty > 0);
                        continue 'trip;
                    }
                }
                bot.close_modal()?;
                bot.pump(3)?;
                return Ok(total);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live 2026-11-20 (81 NoRoute in 24 h, "trade with generalassistant5
    /// failed: walk to spawn (3511,0)", Edgeville and Falador general
    /// stores): the sale site `x,z,level` went as is to `trade_to_keeper`,
    /// whose spawn reads `(level, x, z)`; the keeper walk asked for
    /// (3511,0,0). The site is handed over in the spawn order.
    #[test]
    fn the_sale_site_goes_to_the_keeper_as_a_spawn() {
        assert_eq!(keeper_spawn((3080, 3511, 0)), (0, 3080, 3511));
        assert_eq!(keeper_spawn((3081, 3508, 1)), (1, 3081, 3508));
    }

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

    /// Live 2026-11-19 (alice_swift, alice_knight: "shopsell: trade with
    /// noterazzo failed at (0, 3185, 3436)"): a market delivery paused
    /// the trade at the Bandit Camp, banked the lots at Edgeville and left
    /// the bot at the Varrock square; the sale then clicked for Noterazzo
    /// from Varrock. After a pause the sale takes its lots again and
    /// walks back before any trade; off the site it walks; on the site
    /// with no pause it trades.
    #[test]
    fn a_paused_sale_goes_back_to_its_shop() {
        use crate::kit::site::{pass_leg, PassLeg};
        let camp = Some((3025, 3701, 0));
        let varrock = (3185, 3436, 0);
        assert_eq!(pass_leg(None, 0, false, varrock, camp), PassLeg::Load, "the lots first");
        assert_eq!(pass_leg(Some(3), 3, false, varrock, camp), PassLeg::Walk, "lots carried, off the site");
        assert_eq!(pass_leg(Some(3), 3, false, (3023, 3701, 0), camp), PassLeg::Act, "at the keeper");
        assert_eq!(pass_leg(Some(3), 4, false, varrock, camp), PassLeg::Load, "a pause banked the pack and moved us");
        assert_eq!(pass_leg(Some(3), 4, false, (3023, 3701, 0), camp), PassLeg::Load, "even back on the site: the lots are banked");
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
