#![forbid(unsafe_code)]
//! Future stock bids (plan marché M6, decision Q5 2026-11-16; rules in
//! `lifescape_planner::stock_target`). Every `STOCK_EVERY_SECS` an idle,
//! non-Iron bot (never a quoting market maker) bids ahead, patiently
//! (motive `stock`, horizon 72 000 ticks, `market_lineage`), for what its
//! current work slice and the next one will burn:
//! - consumption per 6000 ticks: the published yield of each slice's
//!   method at the bot's level and tool tier (`rates_read::estimate`, the
//!   same read as every brain consumer: measure blended with the game
//!   prior, the prior alone under n = 3), its consumable outflows (input,
//!   ammo, rune, bait, supply, food); food eaten in the food the bot
//!   affords (`melee_ladder::affordable_food` on the food fitted to its
//!   hitpoints, as the food gate);
//! - slice duration: the tranche's XP at the yield's derived XP per 6000;
//! - held: the free stacks (pack + bank, net of listings and seller
//!   deals) plus the units bought in flight;
//! - unit ceiling: the public value, the current traded price, else the
//!   best ask, never over the cheapest F2P keeper's price (beyond, the
//!   shop is the way, plan marché §3.2), at least the public floor
//!   (credibility §3.6);
//! - budget: `STOCK_BUDGET_PCT` of the free wallet (coins minus every
//!   resting buy and in-flight deal);
//! - an object another motive already bids (a need, gear, a tool) is
//!   left to that bid, an object the alchemy pass claims to it; the
//!   stock objects are never listed for sale
//!   (`kept`, read by `brain_sales`);
//! - withdrawal: a resting `stock` bid whose object left the plan
//!   (`need_gone`) or whose target the holdings reach (`need_met`) is
//!   cancelled, except the alchemy pass's own (`brain_alch::claimed`)
//!   and an object an open obtain node wants (its need takes the
//!   lineage over).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::stock_target::{self, StockLine, StockStep, StockView, WorkSlice};
use lifescape_planner::Life;

use crate::jobs::Job;

/// Wall seconds between two stock passes for one account.
pub const STOCK_EVERY_SECS: f64 = 600.0;

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn kept_table() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Objects the last stock pass of `account` targets (debug names): the
/// sales pass never lists them.
pub fn kept(account: &str) -> Vec<String> {
    kept_table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(account).cloned().unwrap_or_default()
}

/// Combat style of a slice's method (the yield key of a fight carries
/// it; its XP lands in the trained skill).
fn style_of(method: &str, skill: Option<&str>) -> String {
    if method.starts_with("fight:melee/") {
        skill.filter(|skill| matches!(*skill, "attack" | "strength" | "defence")).map(|skill| format!("melee:{skill}")).unwrap_or_default()
    } else if method.starts_with("fight:ranged/") {
        "ranged".to_string()
    } else if method.starts_with("fight:magic/") {
        "magic".to_string()
    } else {
        String::new()
    }
}

/// Consumption per 6000 ticks and work ticks of every slice, read from
/// the published yields `book` for a bot at `level_of` owning `owned`
/// (tool tier); food re-expressed in `food`. A slice without an estimate
/// or a duration is skipped (its typed rate error logged once).
pub fn slice_consumption(book: &crate::rates_estimator::YieldBook, slices: &[WorkSlice], owned: &[String], food: Option<&str>, level_of: &dyn Fn(&str) -> u8, now: i64) -> Vec<(Vec<(String, f64)>, f64)> {
    let mut out = Vec::new();
    for slice in slices {
        let style = style_of(&slice.method, slice.skill.as_deref());
        let level = match &slice.skill {
            Some(_) => slice.from.max(1),
            None => crate::rates_measure::band_skill(&slice.method, &style).map(|skill| level_of(&skill)).unwrap_or(1),
        };
        let tool = crate::report_meta::best_tools(&slice.method, owned).join("+");
        let query = crate::rates_read::RateQuery { method: slice.method.clone(), style, site: None, tool, level };
        let estimate = match crate::rates_read::estimate(book, &query, now) {
            Ok(estimate) => estimate,
            Err(error) => {
                crate::rates_read::log_rate_error(&error);
                continue;
            }
        };
        let xp = slice.skill.as_ref().and_then(|skill| estimate.xp.iter().find(|(name, _)| name == skill).map(|(_, xp)| *xp));
        let Some(ticks) = stock_target::slice_ticks(slice, xp) else { continue };
        let flows: Vec<(String, String, f64)> = estimate
            .flows
            .iter()
            .map(|(display, cause, qty)| (crate::rates_names::debug_in(&slice.method, display, *qty < 0.0), cause.clone(), *qty))
            .collect();
        out.push((stock_target::consumed_per_6000(&flows, food), ticks));
    }
    out
}

/// Unit ceiling of a stock bid: the current traded price, else the best
/// ask, never over the cheapest F2P keeper's price (the shop alone when
/// the book has none); `None` without any public price.
pub fn stock_ceiling(current: Option<i64>, best_ask: Option<i64>, merchant: Option<i64>) -> Option<i64> {
    let book = current.or(best_ask).filter(|price| *price >= 1);
    let ceiling = match (book, merchant.filter(|price| *price >= 1)) {
        (Some(book), Some(shop)) => book.min(shop),
        (Some(book), None) => book,
        (None, shop) => shop?,
    };
    Some(ceiling)
}

/// Resting `stock` bids to cancel under `plan`, with their reason: not
/// the alchemy pass's (`claimed` object ids), not an object an open
/// obtain node wants (`wanted`). Pure.
pub fn stale_stock(rows: &[lifescape_store::Order], plan: &[StockLine], claimed: &BTreeSet<i64>, wanted: &[String], name_of: &dyn Fn(i64) -> Option<String>) -> Vec<(i64, &'static str)> {
    rows.iter()
        .filter(|row| crate::brain_gear::cancellable(row) && row.motive == crate::market_lineage::STOCK && !claimed.contains(&row.item_id))
        .filter_map(|row| {
            let name = name_of(row.item_id)?;
            if wanted.contains(&name) {
                return None;
            }
            stock_target::withdrawal(&name, plan).map(|reason| (row.order_id, reason))
        })
        .collect()
}

/// One stock pass. Returns the bids placed or kept.
pub fn stock_pass(job: &Job, life: &Life, live: &Value, deps: &crate::brain_deps::BrainDeps) -> usize {
    if life.profile.ironman {
        return 0;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0);
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < STOCK_EVERY_SECS) {
            return 0;
        }
        table.insert(job.account.clone(), now);
    }
    let Some(service) = deps.market.clone() else { return 0 };
    let levels = crate::brain_market::live_levels(live);
    if levels.is_empty() {
        return 0;
    }
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    let slices = stock_target::work_slices(life, &level_of);
    let owned = crate::brain_plan::live_owned(live);
    let coins = crate::brain_plan::holdings_count(job, "coins", deps);
    let reader = crate::brain_bids::MarketReader::live(&job.account, deps);
    let price_of = |item: &str| reader.view(item).and_then(|view| view.best_ask().or(view.current));
    let fitted = lifescape_gamedata::melee_ladder::food_to_ask(i64::from(level_of("hitpoints")));
    let food = lifescape_gamedata::melee_ladder::affordable_food(fitted, lifescape_planner::replan::FOOD_BATCH, coins, &price_of);
    let snapshot = deps.rates.clone();
    let consumption = slice_consumption(snapshot.book(), &slices, &owned, Some(&food), &level_of, crate::rates_service::now_secs());
    let mut needs = stock_target::slice_needs(&consumption);
    let (rows, deals) = {
        let store = service.read_store();
        (store.account_orders(&job.account).unwrap_or_default(), store.open_deals_for_account(&job.account).unwrap_or_default())
    };
    let name_of = crate::brain_gear::name_of;
    let id_of = |item: &str| lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64);
    // Another motive's resting bid owns its object (one bid per object),
    // the alchemy pass its own `stock` objects (their ceiling is its).
    let claimed = crate::brain_alch::claimed(&job.account);
    let foreign: BTreeSet<i64> = rows.iter().filter(|row| crate::brain_gear::cancellable(row) && row.motive != crate::market_lineage::STOCK).map(|row| row.item_id).collect();
    needs.retain(|(item, _)| id_of(item).is_none_or(|id| !foreign.contains(&id) && !claimed.contains(&id)));
    let locks = crate::brain_locks::locked_items_in(deps.market.as_deref(), &job.account);
    let free = crate::brain_sales::free_stacks(&crate::brain_plan::live_stacks(job), &|name| crate::brain_locks::locked_in(&locks, name));
    let held = |item: &str| {
        let banked: i64 = free.iter().filter(|(name, _)| name == item).map(|(_, count)| *count).sum();
        let flying: i64 = id_of(item).map(|id| deals.iter().filter(|deal| deal.buyer == job.account && deal.item_id == id).map(|deal| deal.quantity.max(0)).sum()).unwrap_or(0);
        banked + flying
    };
    let resting = |item: &str| {
        id_of(item)
            .map(|id| rows.iter().filter(|row| crate::brain_gear::cancellable(row) && row.motive == crate::market_lineage::STOCK && row.item_id == id).map(|row| row.remaining).sum())
            .unwrap_or(0)
    };
    let ceiling_of = |item: &str| {
        id_of(item)?;
        let view = reader.view(item);
        stock_ceiling(view.as_ref().and_then(|view| view.current), view.as_ref().and_then(|view| view.best_ask()), lifescape_gamedata::obtain::merchant_price(item))
    };
    let budget = crate::brain_gear::free_coins(job, None, deps).saturating_mul(stock_target::STOCK_BUDGET_PCT) / 100;
    let view = StockView { held: &held, resting: &resting, ceiling_of: &ceiling_of, market_affinity: life.profile.market_affinity, budget };
    let plan = stock_target::stock_plan(&needs, &view);
    let mut placed = 0;
    for line in &plan {
        let Some(id) = id_of(&line.item) else { continue };
        let (qty, ceiling) = match line.step {
            StockStep::Bid { qty, ceiling } => (qty, ceiling),
            StockStep::Keep { ceiling: Some(ceiling) } => (1, ceiling),
            _ => continue,
        };
        match crate::market_reprice::keep_bid(&service, &job.account, id, qty, ceiling, crate::market_lineage::STOCK) {
            Ok(crate::market_reprice::StandingBid::Placed { price, .. }) => {
                placed += 1;
                crate::logln!("brain {}: stock {qty} {} bid @ {price} (ceiling {ceiling}, target {}, held {}) for the next work slices", job.account, line.item, line.target, line.held);
            }
            Ok(crate::market_reprice::StandingBid::Kept { .. }) => placed += 1,
            Err(error) => crate::logln!("brain {}: stock {} bid refused: {error}", job.account, line.item),
        }
    }
    let wanted = crate::brain_wishlist::wanted_names(life);
    for (order, reason) in stale_stock(&rows, &plan, &claimed, &wanted, &name_of) {
        let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(&job.account)), ("order", Value::Int(order)), ("reason", str_value(reason))]));
        crate::logln!("brain {}: stock bid {order} withdrawn ({reason}, {:?})", job.account, reply.get("ok").as_bool());
    }
    kept_table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(job.account.clone(), plan.iter().map(|line| line.item.clone()).collect());
    placed
}

#[cfg(test)]
#[path = "brain_stock_tests.rs"]
mod tests;
