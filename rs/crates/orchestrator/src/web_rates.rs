//! Rates HTTP shaping (D44, D46, R2.1): the MEASURES view, one row per
//! measured exact key (signed flows, units, derived XP, confidence: never
//! a copy of a roll-up nor the prior), and the trading-table JSON served
//! beside profit. Thin on purpose: aggregation lives in
//! `rates_service::trading`, this module owns the payload key contract
//! (same column keys as `api_shapes::TradingRow`).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use lifescape_json::{object, str_value, Value};
use lifescape_store::Store;

use super::trading::{trading_rows, trading_value};
use super::{global_snapshot, Snapshot};
use crate::rates_estimator::{Bucket, YieldKey};

/// Trading-table JSON rows for the dashboard payload.
pub fn trading_values(store: &Store, snapshot: &Snapshot) -> Vec<Value> {
    trading_rows(store, &snapshot.trades).iter().map(trading_value).collect()
}

/// The key and confidence columns every measure row repeats.
fn key_fields(bucket: &Bucket, key: &YieldKey, now: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("method", str_value(&key.method)),
        ("style", str_value(&key.style)),
        ("site", str_value(&key.site)),
        ("tool", str_value(&key.tool)),
        ("band", Value::Int(key.band)),
        ("runs", Value::Int(bucket.runs as i64)),
        ("thin", Value::Bool((bucket.runs as usize) < crate::rates_service::MIN_MEASURED_REPORTS)),
        ("age_secs", Value::Int(if bucket.last_clock > 0 { (now - bucket.last_clock).max(0) } else { 0 })),
        ("dispersion", Value::Float(bucket.dispersion())),
        ("failure_rate", Value::Float(bucket.failure_rate())),
        ("rejected", Value::Int(bucket.rejected as i64)),
    ]
}

/// True when `display` is the activity's own product (never folded).
fn own_product(activity: &str, display: &str) -> bool {
    lifescape_planner::method_way::method_way(activity).is_some_and(|(product, _)| crate::rates_names::debug_of(display) == product)
}

/// Rows of the measures view (R2.1): one line per MEASURED exact key
/// (planner activity, style, site, tool, band), its own decayed sums
/// (Σ / Σ work ticks), never the value of another key (the roll-ups stay
/// internal, read fallbacks only) nor the prior; rare pickups fold into
/// one "other drops" line per key (`planner::guide_flows`), valued at
/// the current price (`price`: display name -> coins).
/// (flows: products and consumption, units, derived XP).
pub fn measure_rows(snapshot: &Snapshot, now: i64, price: &mut dyn FnMut(&str) -> f64) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    use lifescape_planner::guide_flows::{fold_negligible_drops, OTHER_DROPS};
    let mut flows = Vec::new();
    let mut units = Vec::new();
    let mut xp = Vec::new();
    for (key, bucket) in snapshot.book.buckets.iter().filter(|(key, bucket)| !key.is_rollup() && bucket.runs > 0) {
        let measured = bucket.flow_rates();
        let mut prices: BTreeMap<String, f64> = BTreeMap::new();
        for (item, _, _) in &measured {
            let unit = price(item);
            prices.insert(item.clone(), unit);
        }
        let unit_price = |item: &str| prices.get(item).copied().unwrap_or(0.0);
        let (kept, other) = fold_negligible_drops(&measured, &|item| own_product(&key.method, item), &unit_price);
        for (item, cause, rate) in &kept {
            let mut fields = key_fields(bucket, key, now);
            fields.extend([("item", str_value(item)), ("cause", str_value(cause)), ("per_6000", Value::Float(*rate)), ("value_6000", Value::Float(rate * unit_price(item)))]);
            flows.push(object(fields));
        }
        if other.items > 0 {
            let mut fields = key_fields(bucket, key, now);
            let label = format!("{OTHER_DROPS} ({} objects)", other.items);
            fields.extend([("item", str_value(&label)), ("cause", str_value("loot")), ("per_6000", Value::Float(other.units_per_6000)), ("value_6000", Value::Float(other.value_per_6000))]);
            flows.push(object(fields));
        }
        let unit_rates = bucket.unit_rates();
        for (kind, subject, rate) in &unit_rates {
            let mut fields = key_fields(bucket, key, now);
            fields.extend([("kind", str_value(kind)), ("subject", str_value(subject)), ("per_6000", Value::Float(*rate))]);
            units.push(object(fields));
        }
        let level = key.band.clamp(1, 99) as u8;
        for (skill, rate) in lifescape_planner::unit_xp::derived_xp(&key.method, &key.style, &crate::rates_names::units_in_debug(&unit_rates), level) {
            let mut fields = key_fields(bucket, key, now);
            fields.extend([("skill", str_value(&skill)), ("per_6000", Value::Float(rate))]);
            xp.push(object(fields));
        }
    }
    (flows, units, xp)
}

/// Dashboard payload: opens the daemon store, serves the estimate of
/// every key (yields and consumption, units, derived XP, confidence,
/// prior or measured), the trading table, plus engaged counts per script counted from
/// the status list. Counts are operator eyes only (this page and API),
/// never read by bots.
pub fn rates_api(call: &dyn Fn(Value) -> Value) -> Value {
    let store = match Store::open(&crate::paths::store_db()) {
        Ok(store) => store,
        Err(error) => {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("rates store: {error}"))),
            ])
        }
    };
    let mut engaged: BTreeMap<String, i64> = BTreeMap::new();
    let status = call(object(vec![("cmd", str_value("status"))]));
    for job in status.get("jobs").as_arr().cloned().unwrap_or_default() {
        if job.get("phase").as_str() != Some("running") {
            continue;
        }
        let script = job.get("script").as_str().unwrap_or("").to_string();
        if script.is_empty() {
            continue;
        }
        *engaged.entry(script).or_insert(0) += 1;
    }
    let shared = global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    rates_payload(&store, &shared, engaged)
}

/// Full rates payload for the dashboard: yield rows, XP rows (profit:
/// the money guide, `/api/guides/money`), the trading table and
/// engaged counts supplied by the caller (operator eyes only, never read
/// by bots).
pub fn rates_payload(store: &Store, snapshot: &Snapshot, engaged: BTreeMap<String, i64>) -> Value {
    let mut prices: BTreeMap<String, f64> = BTreeMap::new();
    let mut price = |item: &str| *prices.entry(item.to_string()).or_insert_with(|| super::price_estimate(store, item).0);
    let (yields, units, xp) = measure_rows(snapshot, crate::rates_service::now_secs(), &mut price);
    let trading: Vec<Value> = trading_values(store, snapshot);
    let mut engaged_map = BTreeMap::new();
    for (script, count) in engaged {
        engaged_map.insert(script, Value::Int(count));
    }
    object(vec![
        ("ok", Value::Bool(true)),
        ("rates", Value::Arr(yields)),
        ("units", Value::Arr(units)),
        ("xp", Value::Arr(xp)),
        ("trading", Value::Arr(trading)),
        ("engaged", Value::Obj(engaged_map)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trading JSON carries the frozen column keys, priced through the
    /// store catalogue (shop price when no market prints exist).
    #[test]
    fn trading_values_carry_frozen_keys() -> Result<(), lifescape_store::Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&lifescape_store::Item {
            item_id: 314,
            name: "Feather".to_string(),
            shop_price: 3,
            alch_value: 1,
        })?;
        let snapshot = {
            let mut snapshot = Snapshot::new();
            snapshot.trades.record_trade(&super::super::ParsedReport {
                account: "t01".to_string(),
                node_id: "n3".to_string(),
                method: "market".to_string(),
                script: "market".to_string(),
                wall_ticks: 6000,
                pause_ticks: 0,
                stop: "budget_exhausted".to_string(),
                level: 1,
                band: 1,
                items: vec![
                    super::super::ParsedItem {
                        name: "Feather".to_string(),
                        provenance: "traded".to_string(),
                        quantity: 100,
                    },
                    super::super::ParsedItem {
                        name: "Coins".to_string(),
                        provenance: "traded".to_string(),
                        quantity: -200,
                    },
                ],
                xp: Vec::new(),
            });
            snapshot
        };
        assert!(trading_values(&store, &snapshot).len() == 1, "book feeds the payload");
        let values = trading_values(&store, &snapshot);
        assert_eq!(values.len(), 1);
        let row = &values[0];
        assert_eq!(row.get("item").as_str(), Some("Feather"));
        assert!((row.get("profit_per_6000").as_f64().unwrap_or(0.0) - -200.0).abs() < 0.001);
        assert_eq!(row.get("deals_settled").as_i64(), Some(1));
        assert!((row.get("fill_rate").as_f64().unwrap_or(0.0) - 1.0).abs() < 0.001);
        assert_eq!(row.get("stock_value").as_i64(), Some(100), "100 untraded feathers at 1 coin");
        Ok(())
    }

    /// Empty book serves an empty table, never an error.
    #[test]
    fn empty_book_serves_empty_table() -> Result<(), lifescape_store::Error> {
        let store = Store::open_in_memory()?;
        let snapshot = Snapshot::new();
        assert!(trading_values(&store, &snapshot).is_empty());
        Ok(())
    }
}
