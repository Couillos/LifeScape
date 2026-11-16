//! Offline check of the loot sales against a COPY of the live data
//! (2026-11-14, frank_lane: combat loot piled up in the bank, never
//! sold). Ignored by default; run with
//!
//! ```text
//! LIFESCAPE_CHECK_DB=<copy of run/lifescape.db> \
//! LIFESCAPE_CHECK_STATUS=<saved /api/status JSON> \
//! LIFESCAPE_CHECK_LIVES=<copy of run/lives> \
//! cargo test -p lifescape-orchestrator loot_check -- --ignored --nocapture
//! ```
//!
//! For every bot of the status, the default sales pass as the brain runs
//! it after this fix, on the copy's book: the resting listings it would
//! regroup (a sliver of a big stack) or bring down to today's opening,
//! then every lot it would list (whole demand lots of its surplus and of
//! its spare gear copies) with price, floor and reserve source, and what
//! it holds. Approximations: no wishlist (tools only, `kept_tools` is
//! applied), no make-cost cap (no live market service), the locks are
//! the copy's resting sells. Read-only on the copies.

use std::collections::{HashMap, HashSet};

use lifescape_json::Value;
use lifescape_market::{LastFill, OwnFill, Reference};
use lifescape_planner::market_fit::{sale_lot, BookView};

use crate::brain_reserve::{CostSource, OwnCosts};
use crate::market_seller::{adoption, plan_sale, undersized, Adoption, FoundMarks, SaleDecision};

/// Debug-name totals of the containers (unstacked pack slots summed,
/// like `brain_plan::live_stacks`).
fn debug_names(job: &Value, keys: &[&str]) -> Vec<(String, i64)> {
    let mut totals: Vec<(String, i64)> = Vec::new();
    for key in keys {
        for entry in job.get(key).as_arr().cloned().unwrap_or_default() {
            let Some(name) = crate::brain_plan::live_debug_name(&entry) else { continue };
            let count = entry.get("count").as_i64().unwrap_or(1);
            match totals.iter_mut().find(|(known, _)| *known == name) {
                Some(slot) => slot.1 += count,
                None => totals.push((name, count)),
            }
        }
    }
    totals
}

fn life_of(account: &str) -> Option<lifescape_planner::Life> {
    let dir = std::env::var("LIFESCAPE_CHECK_LIVES").ok()?;
    let text = std::fs::read_to_string(std::path::Path::new(&dir).join(format!("{account}.json"))).ok()?;
    lifescape_planner::life_from_value(&lifescape_json::parse(&text).ok()?)
}

fn obj_id(name: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// The copy's public book of one item as `account` reads it, its current
/// price, best bid and the cheapest ask of another account.
fn market_of(store: &lifescape_store::Store, resting: &[lifescape_store::Order], account: &str, item: i64) -> (BookView, Reference, Option<i64>, Option<i64>) {
    let clock = crate::market_service::MarketService::now_clock();
    let mut view = BookView { volume_24h: store.volume_between(item, clock - 86_400, clock + 1).unwrap_or(0), ..BookView::default() };
    let mut all_bids: Vec<(i64, i64)> = Vec::new();
    for order in resting.iter().filter(|order| order.item_id == item && order.remaining > 0) {
        if order.side == lifescape_store::OrderSide::Sell {
            view.asks.push((order.price, order.remaining));
        } else {
            all_bids.push((order.price, order.remaining));
            if order.account != account {
                view.bids.push((order.price, order.remaining));
            }
        }
    }
    view.asks.sort_by_key(|(price, _)| *price);
    all_bids.sort_by(|left, right| right.0.cmp(&left.0));
    let fills: Vec<LastFill> = store.last_fills(item, 5).unwrap_or_default().into_iter().map(|(price, quantity)| LastFill { price, quantity }).collect();
    let reference = lifescape_market::current_price(&fills, &all_bids, &view.asks);
    let best_bid = all_bids.first().map(|(price, _)| *price);
    let others_ask = resting.iter().filter(|order| order.item_id == item && order.remaining > 0 && order.side == lifescape_store::OrderSide::Sell && order.account != account).map(|order| order.price).min();
    (view, reference, best_bid, others_ask)
}

fn decide(store: &lifescape_store::Store, resting: &[lifescape_store::Order], costs: &OwnCosts, life: &lifescape_planner::Life, job: &Value, name: &str, item: i64, free: i64) -> (SaleDecision, Option<CostSource>) {
    let account = costs.account.clone();
    let reserve = costs.reserve(store, name, item, Some(life), None);
    let (view, reference, best_bid, others_ask) = market_of(store, resting, &account, item);
    let last = store.last_own_fill(&account, item, false).ok().flatten().map(|(price, _)| OwnFill { price, rested_ticks: None });
    let marks = FoundMarks { others_ask, anchor: lifescape_planner::alchemy::alch_value(name, true), ..FoundMarks::default() };
    let decision = match plan_sale(free, reserve, reference, best_bid, last, marks, &view) {
        SaleDecision::List { qty, price, floor } => match crate::brain_alch::alch_floor(job, name) {
            Some(alch) if alch > floor => SaleDecision::List { qty, price: price.max(alch), floor: alch },
            _ => SaleDecision::List { qty, price, floor },
        },
        hold => hold,
    };
    (decision, reserve.map(|known| known.source))
}

#[test]
#[ignore]
fn loot_check() {
    let (Ok(db), Ok(status)) = (std::env::var("LIFESCAPE_CHECK_DB"), std::env::var("LIFESCAPE_CHECK_STATUS")) else {
        eprintln!("loot_check: set LIFESCAPE_CHECK_DB and LIFESCAPE_CHECK_STATUS");
        return;
    };
    let store = lifescape_store::Store::open(std::path::Path::new(&db)).expect("db copy opens");
    let root = lifescape_json::parse(&std::fs::read_to_string(&status).expect("status json")).expect("status parses");
    let resting = store.resting_orders().expect("orders");
    let only = std::env::var("LIFESCAPE_CHECK_ACCOUNT").ok();
    for job in root.get("jobs").as_arr().cloned().unwrap_or_default() {
        let account = job.get("account").as_str().unwrap_or("?").to_string();
        if only.as_ref().is_some_and(|wanted| *wanted != account) {
            continue;
        }
        let Some(life) = life_of(&account) else { continue };
        if life.profile.ironman {
            continue;
        }
        let stacks = debug_names(&job, &["inv", "bank"]);
        let bank_types = job.get("bank").as_arr().map(|bank| bank.len()).unwrap_or(0);
        let worn: Vec<String> = debug_names(&job, &["worn"]).into_iter().map(|(name, _)| name).collect();
        let levels_owned = crate::brain_market::live_levels(&job);
        let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
        let quests = crate::brain_rescue::quests_done(&life);
        let quests: Vec<&str> = quests.iter().map(String::as_str).collect();
        let mut owned: Vec<String> = stacks.iter().map(|(name, _)| name.clone()).collect();
        owned.extend(worn.iter().cloned());
        let mut gear = lifescape_planner::gear_plan::kept_gear(&levels, &quests, &owned);
        gear.extend(worn.iter().cloned());
        let tools = crate::brain_tools::kept_tools(&life, &stacks, &levels);
        let mut wished = crate::brain_alch::alch_keep(&life, &job);
        wished.extend(tools.iter().cloned());
        let whole = crate::brain_sales::working_keep(&life, &wished);
        let keep = crate::brain_plan::merge_keep(whole.clone(), gear.clone());
        let selling: Vec<lifescape_store::Order> = resting.iter().filter(|order| order.account == account && order.side == lifescape_store::OrderSide::Sell && order.remaining > 0).cloned().collect();
        let mut locked: HashMap<i64, i64> = HashMap::new();
        for row in &selling {
            *locked.entry(crate::market_audit::base_of(row.item_id)).or_default() += row.remaining;
        }
        let free_of = |locked: &HashMap<i64, i64>| -> Vec<(String, i64)> {
            crate::brain_sales::free_stacks(&stacks, &|name| obj_id(name).and_then(|id| locked.get(&id).copied()).unwrap_or(0))
        };
        let mut free = free_of(&locked);
        let costs = OwnCosts::load(&store, &account);
        let combat = costs.reports.iter().any(|report| ["melee", "ranged", "magic"].contains(&report.method.as_str()));
        eprintln!("== {account} ({}, {bank_types} bank types, {} resting sells, {:.2} coins/tick)", if combat { "combat" } else { "other" }, selling.len(), costs.coins_per_tick);
        // Resting listings: regrouped slivers, then today's adoption.
        let mut regrouped: HashSet<i64> = HashSet::new();
        for row in selling.iter().filter(|row| row.status == lifescape_store::OrderStatus::Open) {
            let Some(name) = lifescape_gamedata::obj_by_id(usize::try_from(crate::market_audit::base_of(row.item_id)).unwrap_or(0)).and_then(|obj| obj.name) else { continue };
            if crate::brain_plan::sale_blocked(name, &keep, &[]) {
                continue;
            }
            let spare: i64 = free.iter().filter(|(held, _)| held == name).map(|(_, qty)| *qty).sum();
            let (view, _, _, _) = market_of(&store, &resting, &account, row.item_id);
            if spare > 0 && undersized(row.quantity, row.remaining, spare, sale_lot(row.remaining + spare, Some(&view))) {
                eprintln!("   regroup  {name}: listing {} @{} withdrawn, {} more free", row.remaining, row.price, spare);
                regrouped.insert(row.order_id);
                *locked.entry(crate::market_audit::base_of(row.item_id)).or_default() -= row.remaining;
            }
        }
        free = free_of(&locked);
        for row in selling.iter().filter(|row| !regrouped.contains(&row.order_id)) {
            let Some(name) = lifescape_gamedata::obj_by_id(usize::try_from(crate::market_audit::base_of(row.item_id)).unwrap_or(0)).and_then(|obj| obj.name) else { continue };
            let (today, source) = decide(&store, &resting, &costs, &life, &job, name, row.item_id, row.remaining);
            let reserve = costs.reserve(&store, name, row.item_id, Some(&life), None).map(|reserve| reserve.coins);
            let opening = match today {
                SaleDecision::List { price, .. } => Some(price),
                SaleDecision::Hold(_) => None,
            };
            let (view, _, _, _) = market_of(&store, &resting, &account, row.item_id);
            match adoption(row.price, row.remaining, reserve, sale_lot(row.remaining, Some(&view)), opening) {
                Adoption::Lower { price, floor } => eprintln!("   lower    {name} x{}: {} -> {price} (floor {floor}, {source:?})", row.remaining, row.price),
                Adoption::Raise { price, .. } => eprintln!("   raise    {name} x{}: {} -> {price}", row.remaining, row.price),
                Adoption::Withdraw(why) => eprintln!("   withdraw {name} x{} @{}: {why}", row.remaining, row.price),
                Adoption::Walk { .. } => {}
            }
        }
        // New lots.
        let listed: HashSet<i64> = selling.iter().filter(|row| !regrouped.contains(&row.order_id)).map(|row| row.item_id).collect();
        let mut offers = crate::brain_sales::surplus(&free, &keep, &worn, &listed);
        for (name, qty) in lifescape_planner::gear_plan::spare_gear_units(&free, &gear, &whole) {
            if let Some(id) = obj_id(&name).filter(|id| !listed.contains(id) && crate::brain_sales::tradeable(&name)) {
                offers.push((name, id, qty));
            }
        }
        let (mut lots, mut held) = (0usize, 0usize);
        for (name, item, qty) in offers {
            let (decision, source) = decide(&store, &resting, &costs, &life, &job, &name, item, qty);
            match decision {
                SaleDecision::List { qty: lot, price, floor } => {
                    lots += 1;
                    eprintln!("   list     {lot}/{qty} {name} @{price} (floor {floor}, {source:?})");
                }
                SaleDecision::Hold(why) => {
                    held += 1;
                    eprintln!("   hold     {qty} {name} ({why})");
                }
            }
        }
        let kept: Vec<&String> = stacks.iter().filter(|(name, _)| crate::brain_plan::sale_blocked(name, &keep, &worn)).map(|(name, _)| name).collect();
        eprintln!("   => {lots} new lots, {held} held, {} regrouped; kept whole: {kept:?}", regrouped.len());
    }
}
