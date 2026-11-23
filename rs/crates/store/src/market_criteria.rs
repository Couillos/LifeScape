#![forbid(unsafe_code)]
//! Live criteria of the player market (plan marché §6, lot M0): what
//! `orchestrator economy` prints in its market section. Staff accounts
//! are left out (`excluded`). Lineages come from schema 6 (`replaces`,
//! `root_id`, `root_clock`, `cancel_reason`): an amend is never a
//! cancellation, the age and life of an offer are its lineage's.
//!
//! - two-sided share: objects with resting orders that have both a bid
//!   and an ask; median spread = best ask / best bid over those;
//! - buy lineages that ENDED in the window: settled, or truly cancelled
//!   (the terminal row cancelled, never replaced); `gear_plan` cancels
//!   are the true cancels the gear plan made (C2);
//! - reopened lower: after a truly cancelled buy lineage, the next buy
//!   lineage of the same account and object opens under its last price;
//! - median life of an unserved buy lineage (t0 to its true cancel);
//! - volume (units, coins) of the window's settled executions;
//! - objects that traded at 1 coin while measured consumption (task
//!   flows `input`/`ammo`/`rune`/`bait`/`supply`/`food`, v1 `consumed`
//!   lines) is above 0;
//! - asks at 1 coin; asks-only objects never traded at all;
//! - amends written in the window (CPU of the walk).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::params;

use crate::{Error, Store};

/// Flow causes that consume an object (`orchestrator::flow_causes`).
pub const CONSUMING_CAUSES: [&str; 6] = ["input", "ammo", "rune", "bait", "supply", "food"];
/// Cancel reason the gear plan writes (`brain_gear`, C2).
pub const GEAR_PLAN_REASON: &str = "gear_plan";

/// The §6 numbers over one window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarketCriteria {
    pub active_items: usize,
    pub two_sided_items: usize,
    /// Median of best ask / best bid on two-sided objects.
    pub median_spread: Option<f64>,
    pub buy_settled: usize,
    pub buy_true_cancels: usize,
    pub gear_plan_cancels: usize,
    /// Truly cancelled buy lineages followed by a new lineage of the same
    /// account and object: (opened lower, opened at the same price,
    /// opened higher).
    pub reopened: (usize, usize, usize),
    /// Median seconds from t0 to the true cancel of an unserved buy lineage.
    pub median_unserved_life_secs: Option<i64>,
    pub volume_units: i64,
    pub volume_coins: i64,
    /// Objects traded at 1 coin in the window whose measured consumption > 0.
    pub consumed_at_one: Vec<String>,
    pub ask_one_items: usize,
    pub ask_one_orders: usize,
    pub ask_one_units: i64,
    /// Objects with asks and no bid that never traded.
    pub asks_only_never_traded: usize,
    pub amends: usize,
}

impl MarketCriteria {
    /// Share of active objects quoted on both sides (0 without any).
    pub fn two_sided_share(&self) -> f64 {
        if self.active_items == 0 {
            0.0
        } else {
            self.two_sided_items as f64 / self.active_items as f64
        }
    }

    /// True cancels per settled buy lineage (`None` without a settle).
    pub fn cancel_ratio(&self) -> Option<f64> {
        (self.buy_settled > 0).then(|| self.buy_true_cancels as f64 / self.buy_settled as f64)
    }

    /// Share of reopened lineages that opened lower (`None` without one).
    pub fn reopened_lower_share(&self) -> Option<f64> {
        let (lower, same, higher) = self.reopened;
        let total = lower + same + higher;
        (total > 0).then(|| lower as f64 / total as f64)
    }
}

/// One stored buy row as the lineage pass reads it.
#[derive(Debug, Clone, PartialEq)]
struct BuyRow {
    order_id: i64,
    account: String,
    item_id: i64,
    price: i64,
    status: String,
    updated_clock: i64,
    replaces: Option<i64>,
    root_id: i64,
    root_clock: i64,
    cancel_reason: String,
    /// Price the row opened at (schema 12: before its first in-place amend).
    open_price: i64,
}

/// One buy lineage: opening price, t0, how and when it ended.
#[derive(Debug, Clone, PartialEq)]
struct BuyLineage {
    account: String,
    item_id: i64,
    open_price: i64,
    t0: i64,
    last_price: i64,
    status: String,
    ended: i64,
    cancel_reason: String,
}

fn median_f64(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| left.total_cmp(right));
    Some(values[values.len() / 2])
}

fn lineages_of(rows: Vec<BuyRow>) -> Vec<BuyLineage> {
    let replaced: BTreeSet<i64> = rows.iter().filter_map(|row| row.replaces).collect();
    let mut by_root: BTreeMap<i64, Vec<BuyRow>> = BTreeMap::new();
    for row in rows {
        by_root.entry(row.root_id).or_default().push(row);
    }
    let mut out = Vec::new();
    for (root_id, mut group) in by_root {
        group.sort_by_key(|row| row.order_id);
        let Some(first) = group.first() else { continue };
        let open_price = group.iter().find(|row| row.order_id == root_id).unwrap_or(first).open_price;
        let Some(terminal) = group.iter().rev().find(|row| !replaced.contains(&row.order_id)) else { continue };
        out.push(BuyLineage {
            account: terminal.account.clone(),
            item_id: terminal.item_id,
            open_price,
            t0: if terminal.root_clock > 0 { terminal.root_clock } else { first.updated_clock },
            last_price: terminal.price,
            status: terminal.status.clone(),
            ended: terminal.updated_clock,
            cancel_reason: terminal.cancel_reason.clone(),
        });
    }
    out
}

/// True when `account` is one of `excluded` (any case): staff and the
/// persisted exclusions.
pub(crate) fn excluded_account(excluded: &[String], account: &str) -> bool {
    excluded.iter().any(|name| name.eq_ignore_ascii_case(account))
}

impl Store {
    /// The §6 criteria over `[since, until)` (`until` 0 = now), staff
    /// (`excluded`) left out.
    pub fn market_criteria(&self, since: i64, until: i64, excluded: &[String]) -> Result<MarketCriteria, Error> {
        let mut criteria = MarketCriteria::default();
        self.book_criteria(excluded, &mut criteria)?;
        self.lineage_criteria(since, until, excluded, &mut criteria)?;
        self.trade_criteria(since, until, &mut criteria)?;
        Ok(criteria)
    }

    /// Resting book: two sides, spread, asks at 1, asks-only never traded.
    fn book_criteria(&self, excluded: &[String], criteria: &mut MarketCriteria) -> Result<(), Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT account, side, item_id, price, remaining FROM orders \
             WHERE status IN ('open','matched') AND remaining > 0 AND dormant = 0",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?)))?;
        let mut best: BTreeMap<i64, (Option<i64>, Option<i64>)> = BTreeMap::new();
        let mut ask_one: BTreeSet<i64> = BTreeSet::new();
        for row in rows {
            let (account, side, item_id, price, remaining) = row?;
            if excluded_account(excluded, &account) {
                continue;
            }
            let entry = best.entry(item_id).or_insert((None, None));
            if side == "buy" {
                entry.0 = Some(entry.0.map_or(price, |known| known.max(price)));
            } else {
                entry.1 = Some(entry.1.map_or(price, |known| known.min(price)));
                if price <= 1 {
                    ask_one.insert(item_id);
                    criteria.ask_one_orders += 1;
                    criteria.ask_one_units += remaining;
                }
            }
        }
        criteria.active_items = best.len();
        criteria.ask_one_items = ask_one.len();
        let mut spreads: Vec<f64> = Vec::new();
        let traded: BTreeSet<i64> = {
            let mut traded_statement = self.connection.prepare_cached("SELECT DISTINCT item_id FROM executions")?;
            let ids = traded_statement.query_map([], |row| row.get::<_, i64>(0))?;
            ids.collect::<Result<BTreeSet<i64>, rusqlite::Error>>()?
        };
        for (item_id, (bid, ask)) in &best {
            match (bid, ask) {
                (Some(bid), Some(ask)) => {
                    criteria.two_sided_items += 1;
                    spreads.push(*ask as f64 / (*bid).max(1) as f64);
                }
                (None, Some(_)) if !traded.contains(item_id) => criteria.asks_only_never_traded += 1,
                _ => {}
            }
        }
        criteria.median_spread = median_f64(&mut spreads);
        Ok(())
    }

    /// Buy lineages touched in the window, their ends and their reopenings.
    fn lineage_criteria(&self, since: i64, until: i64, excluded: &[String], criteria: &mut MarketCriteria) -> Result<(), Error> {
        let until = if until <= 0 { i64::MAX } else { until };
        let mut statement = self.connection.prepare_cached(
            "SELECT order_id, account, item_id, price, status, updated_clock, replaces, root_id, root_clock, cancel_reason, \
             COALESCE((SELECT old_price FROM order_amends WHERE order_amends.order_id = orders.order_id ORDER BY amend_id LIMIT 1), price) FROM orders \
             WHERE side = 'buy' AND root_id IN (SELECT DISTINCT root_id FROM orders WHERE side = 'buy' AND (updated_clock >= ?1 OR root_clock >= ?1))",
        )?;
        let mapped = statement.query_map(params![since], |row| {
            Ok(BuyRow {
                order_id: row.get(0)?,
                account: row.get(1)?,
                item_id: row.get(2)?,
                price: row.get(3)?,
                status: row.get(4)?,
                updated_clock: row.get(5)?,
                replaces: row.get(6)?,
                root_id: row.get(7)?,
                root_clock: row.get(8)?,
                cancel_reason: row.get(9)?,
                open_price: row.get(10)?,
            })
        })?;
        let rows: Vec<BuyRow> = mapped.collect::<Result<Vec<BuyRow>, rusqlite::Error>>()?.into_iter().filter(|row| !excluded_account(excluded, &row.account)).collect();
        let lineages = lineages_of(rows);
        let mut next_of: HashMap<(&str, i64), Vec<&BuyLineage>> = HashMap::new();
        for lineage in &lineages {
            next_of.entry((lineage.account.as_str(), lineage.item_id)).or_default().push(lineage);
        }
        for list in next_of.values_mut() {
            list.sort_by_key(|lineage| lineage.t0);
        }
        let mut lives: Vec<f64> = Vec::new();
        for lineage in &lineages {
            let in_window = lineage.ended >= since && lineage.ended < until;
            if !in_window {
                continue;
            }
            match lineage.status.as_str() {
                "settled" => criteria.buy_settled += 1,
                "cancelled" if crate::orders::BOOKKEEPING_REASONS.contains(&lineage.cancel_reason.as_str()) => {}
                "cancelled" => {
                    criteria.buy_true_cancels += 1;
                    if lineage.cancel_reason == GEAR_PLAN_REASON {
                        criteria.gear_plan_cancels += 1;
                    }
                    lives.push((lineage.ended - lineage.t0).max(0) as f64);
                    let following = next_of.get(&(lineage.account.as_str(), lineage.item_id)).and_then(|list| list.iter().find(|next| next.t0 >= lineage.ended));
                    if let Some(next) = following {
                        match next.open_price.cmp(&lineage.last_price) {
                            std::cmp::Ordering::Less => criteria.reopened.0 += 1,
                            std::cmp::Ordering::Equal => criteria.reopened.1 += 1,
                            std::cmp::Ordering::Greater => criteria.reopened.2 += 1,
                        }
                    }
                }
                _ => {}
            }
        }
        criteria.median_unserved_life_secs = median_f64(&mut lives).map(|secs| secs as i64);
        criteria.amends = self.connection.query_row(
            "SELECT (SELECT COUNT(*) FROM orders WHERE replaces IS NOT NULL AND created_clock >= ?1 AND created_clock < ?2) \
                  + (SELECT COUNT(*) FROM order_amends WHERE clock >= ?1 AND clock < ?2)",
            params![since, until],
            |row| row.get::<_, i64>(0),
        )? as usize;
        Ok(())
    }

    /// Volume and the consumed objects that traded at 1 coin.
    fn trade_criteria(&self, since: i64, until: i64, criteria: &mut MarketCriteria) -> Result<(), Error> {
        let (units, coins): (i64, i64) = self.connection.query_row(
            "SELECT COALESCE(SUM(e.quantity), 0), COALESCE(SUM(e.quantity * e.price / COALESCE(i.lot, 1)), 0) FROM executions e LEFT JOIN items i ON i.item_id = e.item_id \
             WHERE e.clock >= ?1 AND (?2 <= 0 OR e.clock < ?2)",
            params![since, until],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        criteria.volume_units = units;
        criteria.volume_coins = coins;
        let mut at_one = self.connection.prepare_cached(
            "SELECT DISTINCT i.name FROM executions e JOIN items i ON i.item_id = e.item_id \
             WHERE e.price <= i.lot AND e.clock >= ?1 AND (?2 <= 0 OR e.clock < ?2) ORDER BY i.name",
        )?;
        let names = at_one.query_map(params![since, until], |row| row.get::<_, String>(0))?.collect::<Result<Vec<String>, rusqlite::Error>>()?;
        let consumed = self.consumed_names(since, until)?;
        criteria.consumed_at_one = names.into_iter().filter(|name| consumed.contains(&name.to_lowercase())).collect();
        Ok(())
    }

    /// Lowercase names of the objects some task consumed in the window.
    fn consumed_names(&self, since: i64, until: i64) -> Result<BTreeSet<String>, Error> {
        let causes = CONSUMING_CAUSES.iter().map(|cause| format!("'{cause}'")).collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT DISTINCT LOWER(f.item_name) FROM task_flows f JOIN task_reports r ON r.report_id = f.report_id \
             WHERE f.cause IN ({causes}) AND f.quantity < 0 AND r.created_clock >= ?1 AND (?2 <= 0 OR r.created_clock < ?2) \
             UNION SELECT DISTINCT LOWER(y.item_name) FROM task_yields y JOIN task_reports r ON r.report_id = y.report_id \
             WHERE y.provenance = 'consumed' AND r.created_clock >= ?1 AND (?2 <= 0 OR r.created_clock < ?2)"
        );
        let mut statement = self.connection.prepare_cached(&query)?;
        let names = statement.query_map(params![since, until], |row| row.get::<_, String>(0))?;
        Ok(names.collect::<Result<BTreeSet<String>, rusqlite::Error>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Error, NewExecution, NewOrder, OrderSide, OrderStatus, Store, AMENDED_REASON};

    const FEATHER: i64 = 314;
    const VAMBRACES: i64 = 1063;
    const HELM: i64 = 1139;
    const LAW: i64 = 563;

    fn order(account: &str, side: OrderSide, item_id: i64, quantity: i64, price: i64, clock: i64) -> NewOrder {
        NewOrder { account: account.to_string(), side, item_id, quantity, price, clock, ..NewOrder::default() }
    }

    /// Fabricated carnet (shapes of the 2026-11-16 constat): feathers on
    /// both sides (ask 3 / bid 2) and consumed by a fly fisher yet traded
    /// at 1; vambraces walked by amends, truly cancelled by the gear plan
    /// and reopened lower; helms asks-only never traded; law runes asks
    /// at 1; one settled buy; staff orders never count.
    #[test]
    fn criteria_of_a_fabricated_carnet() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        for (item_id, name) in [(FEATHER, "Feather"), (VAMBRACES, "Leather vambraces"), (HELM, "Bronze med helm"), (LAW, "Law rune")] {
            store.upsert_item(&crate::Item { item_id, name: name.to_string(), shop_price: 2, alch_value: 1 })?;
        }
        let since = 10_000;
        // Book.
        store.place_order(&order("seller", OrderSide::Sell, FEATHER, 500, 3, since + 1))?;
        store.place_order(&order("fisher", OrderSide::Buy, FEATHER, 100, 2, since + 2))?;
        store.place_order(&order("looter", OrderSide::Sell, HELM, 50, 9, since + 3))?;
        store.place_order(&order("looter", OrderSide::Sell, LAW, 293, 1, since + 4))?;
        store.place_order(&order("admin", OrderSide::Buy, HELM, 1, 900, since + 5))?;
        // Vambraces: root at 1000 (before the window), two amends, a true
        // cancel by the gear plan at since+600, reopened lower 3 s later.
        let root = store.place_order(&NewOrder { motive: "gear".to_string(), ..order("ryan_mason", OrderSide::Buy, VAMBRACES, 1, 10, 1000) })?;
        let mut previous = root;
        for (price, clock) in [(11, since + 100), (12, since + 300)] {
            let next = store.place_order(&NewOrder { replaces: Some(previous), motive: "gear".to_string(), ..order("ryan_mason", OrderSide::Buy, VAMBRACES, 1, price, clock) })?;
            store.cancel_order_because(previous, clock, AMENDED_REASON)?;
            previous = next;
        }
        store.cancel_order_because(previous, since + 600, super::GEAR_PLAN_REASON)?;
        store.place_order(&order("ryan_mason", OrderSide::Buy, VAMBRACES, 1, 10, since + 603))?;
        // A settled buy lineage.
        let settled = store.place_order(&order("smith", OrderSide::Buy, FEATHER, 10, 1, since + 50))?;
        store.set_order_status(settled, OrderStatus::Settled, 0, since + 60)?;
        store.record_execution(&NewExecution { item_id: FEATHER, quantity: 10, price: 1, clock: since + 60, server_tick: 0, taker_side: None })?;
        // Consumption: a fly fisher burnt 40 feathers in the window.
        store.connection.execute_batch(&format!(
            "INSERT INTO task_reports(account, node_id, method, script, ticks, stop, context_level, created_clock, measure_version) \
             VALUES ('fisher', 'n1', 'fishing', 'fishing', 6000, 'goal_reached', 20, {}, 2); \
             INSERT INTO task_flows(report_id, item_name, cause, quantity) VALUES (1, 'Feather', 'bait', -40);",
            since + 70
        ))?;
        let criteria = store.market_criteria(since, 0, &["admin".to_string()])?;
        assert_eq!((criteria.active_items, criteria.two_sided_items), (4, 1), "feather, vambraces, helm, law; staff bid out: {criteria:?}");
        assert_eq!(criteria.median_spread, Some(1.5));
        assert_eq!((criteria.buy_settled, criteria.buy_true_cancels, criteria.gear_plan_cancels), (1, 1, 1), "amends are no cancellation");
        assert_eq!(criteria.reopened, (1, 0, 0), "reopened at 10 under its last 12");
        assert_eq!(criteria.median_unserved_life_secs, Some(since + 600 - 1000), "life from the lineage's t0");
        assert_eq!((criteria.volume_units, criteria.volume_coins), (10, 10));
        assert_eq!(criteria.consumed_at_one, vec!["Feather".to_string()]);
        assert_eq!((criteria.ask_one_items, criteria.ask_one_orders, criteria.ask_one_units), (1, 1, 293));
        assert_eq!(criteria.asks_only_never_traded, 2, "helm and law runes");
        assert_eq!(criteria.amends, 2);
        assert!((criteria.two_sided_share() - 0.25).abs() < 1e-9);
        assert_eq!(criteria.reopened_lower_share(), Some(1.0));
        Ok(())
    }

    /// Schema 12: in-place amends keep one row; the opening price comes
    /// from the journal and the amends are counted from it.
    #[test]
    fn in_place_amends_count_and_keep_the_opening_price() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&crate::Item { item_id: VAMBRACES, name: "Leather vambraces".to_string(), shop_price: 2, alch_value: 1 })?;
        let since = 10_000;
        let walked = store.place_order(&NewOrder { motive: "gear".to_string(), ..order("ryan_mason", OrderSide::Buy, VAMBRACES, 1, 10, 1000) })?;
        for (price, clock) in [(11, since + 100), (12, since + 300)] {
            store.amend_order_in_place(&crate::OrderAmend { order_id: walked, price, remaining: None, ceiling: None, ask_limit: None, clock })?;
        }
        store.cancel_order_because(walked, since + 600, super::GEAR_PLAN_REASON)?;
        store.place_order(&order("ryan_mason", OrderSide::Buy, VAMBRACES, 1, 10, since + 603))?;
        let criteria = store.market_criteria(since, 0, &[])?;
        assert_eq!((criteria.buy_true_cancels, criteria.gear_plan_cancels, criteria.amends), (1, 1, 2));
        assert_eq!(criteria.reopened, (1, 0, 0), "reopened at 10 under its last 12");
        Ok(())
    }
}
