//! Merchant ledger (D44): flip engagements (`market`, `market:*` and
//! `shop:*` methods) never enter the yield/XP rates; their coin P&L lands
//! here, one row per flipped item.
//!
//! One market report carries net flows only, so a same-run buy+sell nets
//! its item to zero and leaves pure coins: such windows inherit the
//! method's last stocked item (exact for single-item flippers, the only
//! shape the flip script runs). Coin P&L with no stocked item ever seen
//! reads `unattributed`, never dropped. `settled` counts clean window
//! exits (`budget_exhausted`, `goal_reached`); anything else lowers the
//! fill rate. Stock positions are ticks-free nets, so they rebuild
//! exactly from the store log; per-window profit samples do not (the
//! store read API carries no per-report ticks) and restart with the
//! snapshot file.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use lifescape_json::{object, str_value, Value};
use lifescape_store::{Store, YieldAggregate};

use super::{aggregate, price_estimate, ParsedReport, NORMALIZE_TICKS};

/// Window exits that count a flip engagement as settled: the clock ran
/// out or the goal landed. `failed`/`stopped` are attempts, not deals.
const SETTLED_STOPS: [&str; 2] = ["budget_exhausted", "goal_reached"];

/// Item placeholder for coin P&L no stocked item explains yet (D45
/// vocabulary): honest, never a yield.
const ORPHAN_ITEM: &str = "unattributed";

/// Market-leg provenances: player trades and NPC shop legs. Incidental
/// gains (`produced`, `loot`, `ground_spawn`) and costs (`consumed`,
/// `lost`) never attract flip P&L or stock.
const LEG_PROVENANCE: [&str; 3] = ["traded", "shop_bought", "shop_sold"];

/// Coins display name, matched case-insensitively: the P&L signal.
pub fn is_coins_item(name: &str) -> bool {
    name.eq_ignore_ascii_case("coins")
}

/// One flipped item: clean exits over engagements, per-window coin P&L
/// samples in arrival order, and raw bought/sold quantities (the net is
/// the position, exact without ticks).
#[derive(Clone, Debug, Default)]
pub struct TradeStat {
    pub reports: usize,
    pub settled: usize,
    pub profits: Vec<f64>,
    pub bought_raw: i64,
    pub sold_raw: i64,
}

/// Whole merchant book: one stat per item plus the last stocked item per
/// method (pure-coin windows inherit it).
#[derive(Clone, Debug, Default)]
pub struct TradeBook {
    items: BTreeMap<String, TradeStat>,
    last_item: BTreeMap<String, String>,
}

impl TradeBook {
    fn leg_flows(parsed: &ParsedReport) -> impl Iterator<Item = &super::ParsedItem> {
        parsed.items.iter().filter(|flow| !is_coins_item(&flow.name) && LEG_PROVENANCE.contains(&flow.provenance.as_str()))
    }

    /// Folds one trading-method engagement: the coin delta is the window
    /// P&L (already net of input costs), booked on the principal leg
    /// item (largest absolute quantity, name tiebreak); each leg flow
    /// credits its own item's stock.
    pub fn record_trade(&mut self, parsed: &ParsedReport) {
        let work_ticks = parsed.wall_ticks.saturating_sub(parsed.pause_ticks);
        if work_ticks <= 0 {
            return;
        }
        let coins: i64 = parsed.items.iter().filter(|flow| is_coins_item(&flow.name)).map(|flow| flow.quantity).sum();
        let per_window = coins as f64 * NORMALIZE_TICKS / work_ticks as f64;
        let mut best: Option<(&str, i64)> = None;
        for flow in Self::leg_flows(parsed) {
            let weight = flow.quantity.abs();
            let replace = match best {
                None => true,
                Some((name, held)) => weight > held || (weight == held && flow.name.as_str() < name),
            };
            if replace {
                best = Some((flow.name.as_str(), weight));
            }
        }
        let item = match best {
            Some((name, _)) => {
                self.last_item.insert(parsed.method.clone(), name.to_string());
                name.to_string()
            }
            None => self.last_item.get(&parsed.method).cloned().unwrap_or_else(|| ORPHAN_ITEM.to_string()),
        };
        let stat = self.items.entry(item).or_default();
        stat.reports += 1;
        if SETTLED_STOPS.contains(&parsed.stop.as_str()) {
            stat.settled += 1;
        }
        stat.profits.push(per_window);
        for flow in Self::leg_flows(parsed) {
            let stock = self.items.entry(flow.name.clone()).or_default();
            if flow.quantity > 0 {
                stock.bought_raw += flow.quantity;
            } else {
                stock.sold_raw += -flow.quantity;
            }
        }
    }

    /// Exact position rebuild from the store log: bought/sold nets need
    /// no ticks. Report counts, settled exits and profit samples are not
    /// restorable (no per-report ticks or stops in the read API) and stay
    /// with the snapshot file.
    pub fn restore_positions(&mut self, aggregates: &[YieldAggregate]) {
        for line in aggregates {
            if !super::is_trading_method(&line.method) || line.item_name.is_empty() || is_coins_item(&line.item_name) {
                continue;
            }
            if !LEG_PROVENANCE.contains(&line.provenance.as_str()) {
                continue;
            }
            let stat = self.items.entry(line.item_name.clone()).or_default();
            if line.total_quantity > 0 {
                stat.bought_raw += line.total_quantity;
            } else {
                stat.sold_raw += -line.total_quantity;
            }
        }
    }

    fn stat_to_value(stat: &TradeStat) -> Value {
        object(vec![
            ("reports", Value::Int(stat.reports as i64)),
            ("settled", Value::Int(stat.settled as i64)),
            ("profits", Value::Arr(stat.profits.iter().map(|sample| Value::Float(*sample)).collect())),
            ("bought", Value::Int(stat.bought_raw)),
            ("sold", Value::Int(stat.sold_raw)),
        ])
    }

    fn stat_from_value(value: &Value) -> TradeStat {
        let profits = value
            .get("profits")
            .as_arr()
            .map(|rows| rows.iter().filter_map(|row| row.as_f64()).collect())
            .unwrap_or_default();
        TradeStat {
            reports: value.get("reports").as_i64().unwrap_or(0).max(0) as usize,
            settled: value.get("settled").as_i64().unwrap_or(0).max(0) as usize,
            profits,
            bought_raw: value.get("bought").as_i64().unwrap_or(0),
            sold_raw: value.get("sold").as_i64().unwrap_or(0),
        }
    }

    /// Schema-2 persistence: positions, samples and last items.
    pub fn to_value(&self) -> (Value, Value) {
        let mut items = BTreeMap::new();
        for (item, stat) in &self.items {
            items.insert(item.clone(), Self::stat_to_value(stat));
        }
        let mut last = BTreeMap::new();
        for (method, item) in &self.last_item {
            last.insert(method.clone(), str_value(item));
        }
        (Value::Obj(items), Value::Obj(last))
    }

    /// Tolerant reload: missing or malformed entries read empty, never fail.
    pub fn from_value(items: &Value, last: &Value) -> TradeBook {
        let mut book = TradeBook::default();
        if let Some(map) = items.as_obj() {
            for (item, entry) in map {
                if item.is_empty() {
                    continue;
                }
                book.items.insert(item.clone(), Self::stat_from_value(entry));
            }
        }
        if let Some(map) = last.as_obj() {
            for (method, item) in map {
                if let Some(item) = item.as_str() {
                    if !method.is_empty() && !item.is_empty() {
                        book.last_item.insert(method.clone(), item.to_string());
                    }
                }
            }
        }
        book
    }
}

/// One published trading row: flip profit per 6000 ticks, settled exits,
/// fill rate and residual stock value (matches `api_shapes::TradingRow`,
/// same column keys).
#[derive(Clone, Debug)]
pub struct TradingRow {
    pub item: String,
    pub profit_per_6000: f64,
    pub deals_settled: i64,
    pub fill_rate: f64,
    pub stock_value: i64,
}

/// Trading rows with estimated prices (24 h VWAP else shop price, same as
/// profit rows). Items with no position and no engagement are skipped.
pub fn trading_rows(store: &Store, book: &TradeBook) -> Vec<TradingRow> {
    let mut rows = Vec::new();
    for (item, stat) in &book.items {
        if stat.reports == 0 && stat.bought_raw == stat.sold_raw {
            continue;
        }
        let (price, _) = price_estimate(store, item);
        rows.push(TradingRow {
            item: item.clone(),
            profit_per_6000: aggregate(&stat.profits, None).mean,
            deals_settled: stat.settled as i64,
            fill_rate: if stat.reports == 0 { 0.0 } else { stat.settled as f64 / stat.reports as f64 },
            stock_value: ((stat.bought_raw - stat.sold_raw) as f64 * price).round() as i64,
        });
    }
    rows.sort_by(|left, right| left.item.cmp(&right.item));
    rows
}

/// Trading rows without prices: tests and offline tools read raw nets.
#[cfg(test)]
pub fn trading_rows_without_store(book: &TradeBook) -> Vec<TradingRow> {
    let mut rows = Vec::new();
    for (item, stat) in &book.items {
        if stat.reports == 0 && stat.bought_raw == stat.sold_raw {
            continue;
        }
        rows.push(TradingRow {
            item: item.clone(),
            profit_per_6000: aggregate(&stat.profits, None).mean,
            deals_settled: stat.settled as i64,
            fill_rate: if stat.reports == 0 { 0.0 } else { stat.settled as f64 / stat.reports as f64 },
            stock_value: stat.bought_raw - stat.sold_raw,
        });
    }
    rows.sort_by(|left, right| left.item.cmp(&right.item));
    rows
}

pub fn trading_value(row: &TradingRow) -> Value {
    object(vec![
        ("item", str_value(&row.item)),
        ("profit_per_6000", Value::Float(row.profit_per_6000)),
        ("deals_settled", Value::Int(row.deals_settled)),
        ("fill_rate", Value::Float(row.fill_rate)),
        ("stock_value", Value::Int(row.stock_value)),
    ])
}

/// Store-backed rebuild: trading positions re-cut from the raw log (see
/// `restore_positions` for what is exact). Yields and XP are not
/// restorable (no per-report ticks in the read API) and stay filed.
pub fn rebuild_from_store(store: &Store) -> Result<super::Snapshot, String> {
    let aggregates = store.aggregate_yields().map_err(|error| format!("rates rebuild store: {error}"))?;
    let mut snapshot = super::Snapshot::new();
    snapshot.trades.restore_positions(&aggregates);
    Ok(snapshot)
}
