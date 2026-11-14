#![forbid(unsafe_code)]
//! Permanent tool kit in the brain (user decision 2026-11-14: every bot,
//! Iron included, owns an axe, a pickaxe, a hammer and a tinderbox, the
//! best tier its level opens, and keeps them across goals). The rule is
//! pure (`lifescape_planner::tool_kit`); this pass files its purchases.
//!
//! Once per `TOOL_SHOP_EVERY_SECS`, with the bank streamed (a banked tool
//! is never bought twice) and no gear step open, the next kit purchase
//! (`tool_kit::kit_purchase`: base kit first, cheapest tools first, then
//! upgrades; wallet shares of the FREE wallet `brain_gear::free_coins`)
//! becomes a gear step (`brain_gear::file_step`): an `obtain_items`
//! resolved `shop:buy` (Nurmof's pickaxes, the general stores' hammer and
//! tinderbox) or, for an axe, `market:buy` on a covered ask else Bob's
//! shop (D-A exception, `gamedata::obtain::kit_shop_route`), plus the
//! `equip_best` that closes the step. Launched before the goal's next
//! node, never blocking it. History: 2026-11-14 the pass bought only
//! pickaxe upgrades for bots whose work needed the family.
//!
//! Loops: one gear step at a time (`gear_ladder::step_open`); a dead
//! step cools its piece down (`WITHDRAWN_RETRY_SECS`, `withdraw_dead`);
//! its failure never bans a skill (`brain_succession`: gear subtree); a
//! shop piece is never withdrawn on an empty book (`brain_launch`). The
//! kit tools are kept by the sales pass (`brain_tools::kept_tools`), the
//! replaced lower tier sells, and the rule only buys above the best
//! working tier: no buy/sell loop. Resting market buys on shop-sold
//! tools (old wishes) are cancelled.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::gear_ladder;
use lifescape_planner::{Life, NodeKind, NodeStatus};

use crate::jobs::Job;

/// Wall seconds between two kit passes for one account.
pub const TOOL_SHOP_EVERY_SECS: f64 = 60.0;

/// True for a gathering tool (pickaxe, woodcutting axe) the kit buys
/// through a step (an NPC shop sells it, `kit_shop_route`): never a
/// market wish bid.
pub fn merchant_tool(name: &str) -> bool {
    let tool = name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"));
    tool && crate::brain_gear::shop_only(name)
}

/// True while an obtain of `item` runs (Active): the kit never buys a
/// second one beside it.
pub fn obtain_in_flight(life: &Life, item: &str) -> bool {
    life.nodes.iter().any(|node| node.status == NodeStatus::Active && matches!(&node.kind, NodeKind::ObtainItems { item: wanted, .. } if wanted == item))
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cancels the account's resting buys on shop-sold tools (old wishlist
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
        crate::logln!("brain {}: wish bid on {name} cancelled ({closed}): the kit buys it through a step", job.account);
    }
}

/// One kit pass. Returns true when the life changed (save it).
pub(crate) fn tool_shop_pass(job: &Job, life: &mut Life, live: &Value) -> bool {
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < TOOL_SHOP_EVERY_SECS) {
            return false;
        }
        table.insert(job.account.clone(), now);
    }
    if !life.profile.ironman {
        cancel_merchant_tool_bids(job, life);
    }
    // A bank not streamed yet hides banked tools: never buy a second one.
    if gear_ladder::step_open(life) || live.get("bank_streamed").as_bool() != Some(true) {
        return false;
    }
    let levels_owned = crate::brain_tools::live_levels(live);
    if levels_owned.is_empty() {
        return false;
    }
    let levels = crate::brain_tools::level_refs(&levels_owned);
    let (owned, _) = crate::brain_gear::holdings(job);
    let coins = crate::brain_gear::free_coins(job, None);
    let purchases = lifescape_planner::tool_kit::kit_purchases(&levels, &owned, coins);
    let Some(purchase) = purchases
        .into_iter()
        .find(|purchase| !obtain_in_flight(life, &purchase.item) && !crate::brain_gear::recently_withdrawn(&job.account, &purchase.item, now))
    else {
        return false;
    };
    let kind = if purchase.base { "missing kit tool" } else { "kit upgrade" };
    let why = format!("{kind}, shop price {}, free wallet {coins}", purchase.price);
    crate::brain_gear::file_step(job, life, Some(&purchase.item), crate::brain_gear::goal_style(life), &why);
    true
}

#[cfg(test)]
#[path = "brain_tool_shop_tests.rs"]
mod tests;
