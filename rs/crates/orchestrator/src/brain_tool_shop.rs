#![forbid(unsafe_code)]
//! Merchant tools (user 2026-11-14: "personne n'a mieux qu'une bronze
//! pickaxe"). No F2P recipe smiths a pickaxe in rev 274 and no F2P
//! monster drops one: iron to rune pickaxes come from Nurmof's shop
//! (Dwarven Mine, `pickaxeshop`) only. The wishlist bid on them on the
//! market (185 bids, never filled: no bot sells one). Here a tool whose
//! upgrade has a merchant route (`gamedata::obtain::merchant_route`, D-A)
//! becomes a gear step instead, the equipment precedent (`brain_gear`):
//! an `obtain_items` resolved `shop:buy` + the `equip_best` that closes
//! the step, launched before the goal's next node, never blocking it.
//!
//! Rule (`tool_purchase`, pure): for a tool family the bot's work needs
//! (`brain_tools::work_needs`: the same predicate the sales pass keeps
//! tools by, so a bought tool is never listed while it is useful), the
//! best merchant tool its level opens that beats the best usable one it
//! owns and whose shop price (`merchant_price`, the server formula) fits
//! `TOOL_SHARE_PCT` (25 %) of its free wallet (`brain_gear::free_coins`).
//! A thin wallet takes the best tier it pays (steel before rune); no
//! affordable upgrade, nothing filed (the bot earns, the next pass looks
//! again). Only an upgrade of a family it already uses: a missing tool
//! is the graph's own obtain.
//!
//! Loops: one gear step at a time (`gear_ladder::step_open`); a dead
//! step cools its piece down (`WITHDRAWN_RETRY_SECS`, `withdraw_dead`);
//! its failure never bans a skill (`brain_succession`: gear subtree); a
//! shop piece is never withdrawn on an empty book (`brain_launch`). The
//! old tool is surplus (sold by the sales pass) and never wished again:
//! the rule only buys above the best owned tier. Resting market buys on
//! merchant tools (old wishes) are cancelled.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::gear_ladder::{self, TOOL_SHARE_PCT};
use lifescape_planner::Life;

use crate::jobs::Job;

/// Wall seconds between two tool shop passes for one account.
pub const TOOL_SHOP_EVERY_SECS: f64 = 60.0;

/// Tool families: gear family, canonical name in the work needs.
const FAMILIES: [(&str, &str); 2] = [("pickaxe", "bronze_pickaxe"), ("axe", "bronze_axe")];

/// One shop purchase: the tool and its shop price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPurchase {
    pub item: String,
    pub price: i64,
}

/// The best merchant tool of `family` (`pickaxe`, `axe`) the bot's
/// `levels` open, better than the best usable tool of that family it
/// `owned` (pack, bank, worn), whose shop price fits `ceiling`. `None`
/// when it owns none (the graph obtains a first tool), already holds the
/// best, or no better merchant tier fits the wallet. Pure.
pub fn tool_purchase(family: &str, levels: &[(&str, i64)], owned: &[String], ceiling: i64) -> Option<ToolPurchase> {
    let usable = lifescape_gamedata::gear::usable_tools(family, levels);
    let owned_rank = usable.iter().position(|name| owned.contains(name))?;
    usable[..owned_rank]
        .iter()
        .filter(|name| lifescape_gamedata::obtain::merchant_route(name))
        .filter_map(|name| lifescape_gamedata::obtain::merchant_price(name).map(|price| ToolPurchase { item: name.clone(), price }))
        .find(|purchase| purchase.price <= ceiling)
}

/// True for a gathering tool (pickaxe, woodcutting axe) only an NPC shop
/// sells (D-A): no market bid ever fills for it.
pub fn merchant_tool(name: &str) -> bool {
    let tool = name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"));
    tool && crate::brain_gear::shop_only(name)
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cancels the account's resting buys on merchant tools (old wishlist
/// bids nobody fills), except what the graph's own obtains want.
fn cancel_merchant_tool_bids(job: &Job, life: &Life) {
    let Some(service) = crate::market_service::shared() else { return };
    let spared = crate::brain_wishlist::wanted_names(life);
    let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
    let mut names: Vec<String> = rows
        .iter()
        .filter(|row| crate::brain_gear::cancellable(row))
        .filter_map(|row| crate::brain_gear::name_of(row.item_id))
        .filter(|name| merchant_tool(name) && !spared.contains(name))
        .collect();
    names.sort();
    names.dedup();
    for name in &names {
        let closed = crate::brain_market::cancel_open_buys(&job.account, name);
        crate::logln!("brain {}: wish bid on {name} cancelled ({closed}): only a shop sells it, bought there", job.account);
    }
}

/// One tool shop pass. Returns true when the life changed (save it).
pub(crate) fn tool_shop_pass(job: &Job, life: &mut Life, live: &Value) -> bool {
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < TOOL_SHOP_EVERY_SECS) {
            return false;
        }
        table.insert(job.account.clone(), now);
    }
    cancel_merchant_tool_bids(job, life);
    if gear_ladder::step_open(life) {
        return false;
    }
    let levels_owned = crate::brain_market::live_levels(live);
    if levels_owned.is_empty() {
        return false;
    }
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let (needed, _) = crate::brain_tools::work_needs(life);
    let (owned, _) = crate::brain_gear::holdings(job);
    let stepping = crate::brain_wishlist::wanted_names(life);
    let coins = crate::brain_gear::free_coins(job, None);
    let ceiling = gear_ladder::wallet_ceiling(coins, TOOL_SHARE_PCT);
    for (family, canonical) in FAMILIES {
        if !needed.iter().any(|tool| tool == canonical) {
            continue;
        }
        let Some(purchase) = tool_purchase(family, &levels, &owned, ceiling) else { continue };
        if stepping.contains(&purchase.item) || crate::brain_gear::recently_withdrawn(&job.account, &purchase.item, now) {
            continue;
        }
        let why = format!("{family} upgrade at the shop, price {}, ceiling {ceiling} of wallet {coins}", purchase.price);
        crate::brain_gear::file_step(job, life, Some(&purchase.item), crate::brain_gear::goal_style(life), &why);
        return true;
    }
    false
}

#[cfg(test)]
#[path = "brain_tool_shop_tests.rs"]
mod tests;
