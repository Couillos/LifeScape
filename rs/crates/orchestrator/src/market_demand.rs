#![forbid(unsafe_code)]
//! Demand signals (read-only, public market data only, D22): what the
//! open buy orders ask for, by item, and what each production chain pays.
//! The ambitions read them (draw weights, demand milestones: a bid makes
//! some bot produce it, even training or buying inputs first); agent E's
//! sales and wishes may read them too.
//!
//! API:
//! - `signals()`: one `DemandSignal` per item with resting bids — bid
//!   count, distinct bidders, unmet quantity, best bid, age of the oldest
//!   bid (seconds and ticks), and a relative `weight` = best bid × unmet ×
//!   (1 + min(age / 6000 ticks, 5)): an old unmet bid pulls harder. The
//!   age is the LINEAGE's (t0 kept by every amend, plan marché M1 C1): a
//!   bid walked for 6 h is 6 h old, not one patience window.
//!   Empty while the market service is down (no data, no decision).
//! - `kind_profits()`: best profit per ambition kind, read in the
//!   precomputed money guide (`brain_d17::kind_profits`: its catalogue
//!   rows, chains included, under the kind of their band skill); none
//!   before the first set.
//! - `surplus_from()`: units resting on the asks per object (market
//!   makers' quotes left out), the need's outlet signal
//!   (`ambition_catalog::kind_weights_with`, user 2026-11-19).
//! - `demand_items()` / `kind_demand()`: the planner inputs; each item
//!   carries its per-unit `margin` at the best bid against its inputs at
//!   their current market price (`planner::chain_profit`; median of the
//!   last five fills clamped by the book, ending at 1 coin): only a positive margin
//!   makes a demand milestone (review 2026-11-13: a 1-coin bid made a
//!   smith forge steel platebodies at a loss).

use std::collections::BTreeMap;

use lifescape_planner::ambition_catalog::kind_weights_with;
use lifescape_planner::{DemandItem, KindDemand};
use lifescape_store::{Order, OrderSide};

/// Ticks over which a bid's age doubles its pull (capped at ×6).
pub const AGE_UNIT_TICKS: u64 = 6000;

/// Open demand for one item (resting buy orders).
#[derive(Debug, Clone, PartialEq)]
pub struct DemandSignal {
    /// Debug name (`bronze_platebody`).
    pub item: String,
    pub item_id: i64,
    pub open_bids: usize,
    pub bidders: usize,
    pub unmet_qty: i64,
    pub best_bid: i64,
    pub oldest_age_secs: i64,
    pub oldest_age_ticks: u64,
    pub weight: f64,
    /// Units on resting sell offers of the item (public book, any seller,
    /// market makers included): 0 = wanted and missing from the asks.
    pub asks: i64,
}

/// Units on resting sell offers per item id (every seller, makers too).
fn ask_units(orders: &[Order]) -> BTreeMap<i64, i64> {
    let mut asked: BTreeMap<i64, i64> = BTreeMap::new();
    for order in orders.iter().filter(|order| order.side == OrderSide::Sell && order.remaining > 0) {
        *asked.entry(order.item_id).or_default() += order.remaining;
    }
    asked
}

/// Pure aggregation of resting orders (buy side, remaining > 0). A market
/// maker's bid (plan Q6, motive `market_make`) is liquidity, not
/// consumption: it never reads as demand (ambitions, bid responses).
pub fn signals_from(orders: &[Order], now_secs: i64, name_of: &dyn Fn(i64) -> Option<String>, to_ticks: &dyn Fn(i64) -> u64) -> Vec<DemandSignal> {
    let mut by_item: BTreeMap<i64, Vec<&Order>> = BTreeMap::new();
    for order in orders.iter().filter(|order| order.side == OrderSide::Buy && order.remaining > 0 && order.motive != crate::market_lineage::MARKET_MAKE) {
        by_item.entry(order.item_id).or_default().push(order);
    }
    let asked = ask_units(orders);
    let mut out = Vec::new();
    for (item_id, bids) in by_item {
        let Some(item) = name_of(item_id) else { continue };
        let mut bidders: Vec<&str> = bids.iter().map(|order| order.account.as_str()).collect();
        bidders.sort_unstable();
        bidders.dedup();
        let unmet_qty: i64 = bids.iter().map(|order| order.remaining).sum();
        // Per unit (plan prix lot D: the carnet holds coins per lot).
        let best_bid = bids.iter().map(|order| crate::market_units::to_unit(item_id, order.price)).max().unwrap_or(0);
        let oldest_age_secs = bids.iter().map(|order| (now_secs - order.lineage_clock()).max(0)).max().unwrap_or(0);
        let oldest_age_ticks = to_ticks(oldest_age_secs);
        let age_factor = 1.0 + (oldest_age_ticks as f64 / AGE_UNIT_TICKS as f64).min(5.0);
        out.push(DemandSignal {
            item,
            item_id,
            open_bids: bids.len(),
            bidders: bidders.len(),
            unmet_qty,
            best_bid,
            oldest_age_secs,
            oldest_age_ticks,
            weight: best_bid.max(1) as f64 * unmet_qty as f64 * age_factor,
            asks: asked.get(&item_id).copied().unwrap_or(0),
        });
    }
    out.sort_by(|left, right| right.weight.total_cmp(&left.weight));
    out
}

/// Orders of every account but `account` (a bot never answers its own
/// bids: live 2026-11-13, b3gybcev produced raw_trout for its own bid).
pub fn others_orders(orders: Vec<Order>, account: &str) -> Vec<Order> {
    orders.into_iter().filter(|order| !order.account.eq_ignore_ascii_case(account)).collect()
}

/// Live demand signals from the daemon's market (heaviest first), the
/// whole fleet's (draw weights, fleet assignment).
pub fn signals() -> Vec<DemandSignal> {
    signals_seen_by(None)
}

/// Live demand signals one bot may answer: every resting bid but its own
/// (its own wishes and graph buys included: they are its orders).
pub fn signals_for(account: &str) -> Vec<DemandSignal> {
    signals_seen_by(Some(account))
}

/// Both planner inputs as one bot sees them (own bids left out).
pub fn kind_demand_for(account: &str) -> (Vec<DemandItem>, Vec<KindDemand>) {
    kind_demand_seen_by(Some(account))
}

/// Both planner inputs from ONE read of the book: the bids `account`
/// may answer (`None`: the fleet's) and the surplus on the asks (the
/// outlet signal, `surplus_from`).
fn kind_demand_seen_by(account: Option<&str>) -> (Vec<DemandItem>, Vec<KindDemand>) {
    let orders = resting();
    let surplus = surplus_from(&orders, &name_of);
    let items = demand_items(&signals_seen_from(orders, account, crate::market_service::MarketService::now_clock(), &name_of, &crate::brain_stuck::seconds_to_ticks));
    let weights = kind_weights_with(&items, &kind_profits(), &surplus);
    (items, weights)
}

/// Debug name of a book item.
fn name_of(item_id: i64) -> Option<String> {
    lifescape_gamedata::obj_by_id(item_id.max(0) as usize).and_then(|obj| obj.name).map(str::to_string)
}

/// Resting orders of the daemon's market (empty while it is down).
fn resting() -> Vec<Order> {
    let Some(service) = crate::market_service::shared() else { return Vec::new() };
    let read = service.lock_store().resting_orders();
    match read {
        Ok(rows) => rows,
        Err(error) => {
            crate::logln!("market_demand: resting orders unreadable: {error}");
            Vec::new()
        }
    }
}

/// Units resting on the asks per object (debug name), the outlet signal
/// of the need (user 2026-11-19: what piles up unused calls for the
/// skills that consume it). A market maker's ask is liquidity, not a
/// surplus (like its bids, `signals_from`).
pub fn surplus_from(orders: &[Order], name_of: &dyn Fn(i64) -> Option<String>) -> Vec<(String, f64)> {
    let mut by_item: BTreeMap<i64, i64> = BTreeMap::new();
    for order in orders.iter().filter(|order| order.side == OrderSide::Sell && order.remaining > 0 && order.motive != crate::market_lineage::MARKET_MAKE) {
        *by_item.entry(order.item_id).or_default() += order.remaining;
    }
    by_item.into_iter().filter_map(|(item_id, units)| Some((name_of(item_id)?, units as f64))).collect()
}

fn signals_seen_by(account: Option<&str>) -> Vec<DemandSignal> {
    signals_seen_from(resting(), account, crate::market_service::MarketService::now_clock(), &name_of, &crate::brain_stuck::seconds_to_ticks)
}

/// Pure `signals_seen_by`: the bids of every account but `account` (a bot
/// never answers its own bids); `asks` counts EVERY resting sell offer,
/// the bot's own included (lot 6 review: an object only this bot sells
/// is not « without asks » for it).
pub fn signals_seen_from(orders: Vec<Order>, account: Option<&str>, now_secs: i64, name_of: &dyn Fn(i64) -> Option<String>, to_ticks: &dyn Fn(i64) -> u64) -> Vec<DemandSignal> {
    let asked = ask_units(&orders);
    let orders = match account {
        Some(account) => others_orders(orders, account),
        None => orders,
    };
    let mut out = signals_from(&orders, now_secs, name_of, to_ticks);
    for signal in &mut out {
        signal.asks = asked.get(&signal.item_id).copied().unwrap_or(0);
    }
    out
}

/// Planner input: one `DemandItem` per signal, its margin priced at the
/// market reference of every input (one store read for the whole list).
/// Store down: no margin, so no demand milestone (no data, no decision).
pub fn demand_items(signals: &[DemandSignal]) -> Vec<DemandItem> {
    if signals.is_empty() {
        return Vec::new();
    }
    match lifescape_store::Store::open(&crate::market_service::db_path()) {
        Ok(store) => demand_items_with(signals, &|item: &str| crate::rates_service::price_estimate(&store, &display_of(item)).0),
        Err(error) => {
            crate::logln!("market_demand: store unreadable, bids carry no margin: {error}");
            demand_items_with(signals, &|_| f64::NAN)
        }
    }
}

/// Pure `demand_items`: margin = best bid minus the inputs of the best
/// production way at `reference` (debug name → current price: median of
/// the last five fills clamped by the book, else the book, else 1 coin;
/// never a shop price), per unit
/// (`planner::chain_profit`). An input with no reference (NaN) or an
/// object no production way makes leaves the margin `None`. Inputs are
/// costed at the reference even when the producer gathers them (the
/// opportunity cost), not at the best ask.
pub fn demand_items_with(signals: &[DemandSignal], reference: &dyn Fn(&str) -> f64) -> Vec<DemandItem> {
    signals
        .iter()
        .map(|signal| DemandItem { item: signal.item.clone(), weight: signal.weight, unmet: signal.unmet_qty, margin: bid_margin(signal, reference), asked: signal.asks > 0 })
        .collect()
}

fn bid_margin(signal: &DemandSignal, reference: &dyn Fn(&str) -> f64) -> Option<f64> {
    let mut prices = vec![(signal.item.clone(), signal.best_bid.max(0) as f64)];
    for way in lifescape_gamedata::obtain::ways(&signal.item).into_iter().filter(|way| way.is_production()) {
        for (input, _) in &way.inputs {
            if !prices.iter().any(|(known, _)| known == input) {
                prices.push((input.clone(), reference(input)));
            }
        }
    }
    lifescape_planner::chain_profit(&signal.item, &prices, 1.0).ok().map(|verdict| verdict.margin)
}

fn display_of(item: &str) -> String {
    lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item).to_string()
}

/// Best profit per ambition kind: the money guide's (module doc).
pub fn kind_profits() -> Vec<(String, f64)> {
    crate::guides_store::current().map(|set| crate::brain_d17::kind_profits(&set)).unwrap_or_default()
}

/// Both planner inputs at once: demand items and kind weights.
pub fn kind_demand() -> (Vec<DemandItem>, Vec<KindDemand>) {
    kind_demand_seen_by(None)
}

#[cfg(test)]
mod tests {
    use super::{demand_items, signals_from};
    use lifescape_store::{Order, OrderSide, OrderStatus};

    fn bid(account: &str, item: i64, remaining: i64, price: i64, created: i64) -> Order {
        Order {
            order_id: 1,
            account: account.to_string(),
            side: OrderSide::Buy,
            item_id: item,
            quantity: remaining,
            remaining,
            price,
            status: OrderStatus::Open,
            dormant: false,
            created_clock: created,
            updated_clock: created,
            ..Order::default()
        }
    }

    /// Plan marché C1/M1: a bid walked by amends for 6 h (each step a
    /// new row created a moment ago) is 6 h old: ×6, not ×1.
    #[test]
    fn signals_read_the_age_of_the_lineage() {
        let now = 100_000;
        let mut walked = bid("ryan_mason", 1, 1, 26, now - 30);
        walked.root_id = 7;
        walked.root_clock = now - 6 * 3600;
        let name = |id: i64| Some(format!("item{id}"));
        // 60 ms ticks (world.json): 6 h = 360 000 ticks.
        let ticks = |secs: i64| (secs * 1000 / 60) as u64;
        let found = signals_from(&[walked], now, &name, &ticks);
        assert_eq!(found[0].oldest_age_secs, 6 * 3600);
        assert!((found[0].weight - 26.0 * 1.0 * 6.0).abs() < 1e-9, "age factor capped at x6: {}", found[0].weight);
    }

    /// The outlet signal: units on the asks per object, every seller but
    /// a market maker (its quote is liquidity, not a surplus).
    #[test]
    fn surplus_counts_the_asks_but_never_a_makers_quote() {
        let ask = |account: &str, item: i64, units: i64| Order { side: OrderSide::Sell, ..bid(account, item, units, 5, 0) };
        let mut quote = ask("maker_one", 1, 500);
        quote.motive = crate::market_lineage::MARKET_MAKE.to_string();
        let orders = vec![ask("woodcutter", 1, 900), ask("other", 1, 100), quote, bid("firemaker", 1, 50, 4, 0), ask("priest", 2, 30)];
        let name = |id: i64| Some(format!("item{id}"));
        assert_eq!(super::surplus_from(&orders, &name), vec![("item1".to_string(), 1000.0), ("item2".to_string(), 30.0)]);
    }

    /// Q6: a maker's bid on every object never turns into demand.
    #[test]
    fn a_market_makers_bid_is_no_demand() {
        let mut quote = bid("maker_one", 1, 500, 2, 0);
        quote.motive = crate::market_lineage::MARKET_MAKE.to_string();
        let name = |id: i64| Some(format!("item{id}"));
        let ticks = |secs: i64| secs as u64;
        assert!(signals_from(&[quote.clone()], 10, &name, &ticks).is_empty());
        assert_eq!(signals_from(&[quote, bid("fisher", 1, 5, 3, 0)], 10, &name, &ticks)[0].unmet_qty, 5, "the consumer's bid only");
    }

    #[test]
    fn signals_aggregate_bids_by_item_and_age() {
        let mut ask = bid("s", 1, 5, 10, 0);
        ask.side = OrderSide::Sell;
        let orders = vec![bid("a", 1, 10, 50, 0), bid("b", 1, 5, 60, 900), bid("a", 2, 3, 5, 990), ask];
        let name = |id: i64| Some(format!("item{id}"));
        let ticks = |secs: i64| (secs * 10) as u64;
        let found = signals_from(&orders, 1000, &name, &ticks);
        assert_eq!(found.len(), 2, "sell side ignored");
        let first = &found[0];
        assert_eq!((first.item.as_str(), first.open_bids, first.bidders, first.unmet_qty, first.best_bid), ("item1", 2, 2, 15, 60));
        assert_eq!((first.oldest_age_secs, first.oldest_age_ticks), (1000, 10_000));
        assert!((first.weight - 60.0 * 15.0 * (1.0 + 10_000.0 / 6000.0)).abs() < 1e-6);
        assert_eq!(demand_items(&found)[0].unmet, 15);
    }

    /// Live 2026-11-13 (b3gybcev): its own bid raw_trout 47 @1 became its
    /// side goal "produce raw_trout 47", its own bronze_axe bid a quota.
    /// The demand a bot reads leaves out its own orders.
    #[test]
    fn a_bot_never_reads_its_own_bids() {
        let orders = vec![bid("b3gybcev", 1, 47, 1, 0), bid("other", 2, 5, 9, 0), bid("b3gybcev", 2, 3, 9, 0)];
        let name = |id: i64| Some(format!("item{id}"));
        let ticks = |secs: i64| secs as u64;
        let seen = signals_from(&super::others_orders(orders.clone(), "b3gybcev"), 10, &name, &ticks);
        let items: Vec<(&str, i64)> = seen.iter().map(|signal| (signal.item.as_str(), signal.unmet_qty)).collect();
        assert_eq!(items, vec![("item2", 5)], "own bids gone, the other buyer's stays");
        assert_eq!(signals_from(&orders, 10, &name, &ticks).len(), 2, "the fleet view keeps every bid");
    }

    /// Lot 6 review: `asks` counts every resting sell offer of the item
    /// (makers included), the reading bot's own too; its own BIDS stay out.
    #[test]
    fn asks_count_every_resting_sell_offer() {
        let mut own_ask = bid("seller", 2, 5, 40, 0);
        own_ask.side = OrderSide::Sell;
        let mut maker_ask = bid("maker", 3, 7, 40, 0);
        maker_ask.side = OrderSide::Sell;
        maker_ask.motive = crate::market_lineage::MARKET_MAKE.to_string();
        let orders = vec![bid("other", 2, 4, 30, 0), own_ask, bid("seller", 3, 2, 30, 0), bid("other", 3, 1, 30, 0), maker_ask];
        let name = |id: i64| Some(format!("item{id}"));
        let ticks = |secs: i64| secs as u64;
        let asks = |signals: &[super::DemandSignal], item: &str| signals.iter().find(|signal| signal.item == item).map(|signal| signal.asks);
        let fleet = signals_from(&orders, 10, &name, &ticks);
        assert_eq!((asks(&fleet, "item2"), asks(&fleet, "item3")), (Some(5), Some(7)), "{fleet:?}");
        let seen = super::signals_seen_from(orders, Some("seller"), 10, &name, &ticks);
        assert_eq!(asks(&seen, "item2"), Some(5), "its own ask still supplies the object: {seen:?}");
        let item3 = seen.iter().find(|signal| signal.item == "item3").expect("the other bid");
        assert_eq!((item3.unmet_qty, item3.asks), (1, 7), "own bid left out, the maker's ask counted");
    }

    /// Review 2026-11-13 (finding 5): a bid only motivates production when
    /// it pays the inputs at their market reference: a steel platebody at
    /// 1 coin (5 bars) never does, a bronze one at 200 does, a gathered
    /// fish pays its whole bid, an object nobody makes has no margin.
    #[test]
    fn margins_price_the_best_bid_against_input_references() {
        let signal = |item: &str, best_bid: i64| super::DemandSignal {
            item: item.to_string(),
            item_id: 0,
            open_bids: 1,
            bidders: 1,
            unmet_qty: 5,
            best_bid,
            oldest_age_secs: 0,
            oldest_age_ticks: 0,
            weight: 1.0,
            asks: 0,
        };
        let reference = |item: &str| match item {
            "bronze_bar" => 10.0,
            _ => 1.0,
        };
        let rows = [signal("steel_platebody", 1), signal("bronze_platebody", 200), signal("raw_shrimp", 3), signal("shears", 50)];
        let items = super::demand_items_with(&rows, &reference);
        let margin = |item: &str| items.iter().find(|row| row.item == item).and_then(|row| row.margin);
        assert!(margin("steel_platebody").is_some_and(|value| value < 0.0), "{items:?}");
        assert!(margin("bronze_platebody").is_some_and(|value| (value - 150.0).abs() < 1e-6), "{items:?}");
        assert!(margin("raw_shrimp").is_some_and(|value| value > 0.0), "{items:?}");
        assert_eq!(margin("shears"), None, "bought from shops only: no production margin");
    }
}
