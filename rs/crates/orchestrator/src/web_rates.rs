//! Rates HTTP shaping (D44, D46, R2): per-key estimates (signed flows,
//! units, derived XP, confidence) and the trading-table JSON served
//! beside profit. Thin on purpose: aggregation lives in
//! `rates_service::trading`, this module owns the payload key contract
//! (same column keys as `api_shapes::TradingRow`).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use lifescape_json::{object, str_value, Value};
use lifescape_store::Store;

use super::trading::{trading_rows, trading_value};
use super::{global_snapshot, profit_rows, ProfitRow, Snapshot};
use crate::rates_estimator::YieldKey;
use crate::rates_read::RateEstimate;

/// Trading-table JSON rows for the dashboard payload.
pub fn trading_values(store: &Store, snapshot: &Snapshot) -> Vec<Value> {
    trading_rows(store, &snapshot.trades).iter().map(trading_value).collect()
}

fn profit_value(row: &ProfitRow) -> Value {
    object(vec![
        ("method", str_value(&row.method)),
        ("style", str_value(&row.style)),
        ("band", Value::Int(row.band)),
        ("revenue", Value::Float(row.revenue)),
        ("input_cost", Value::Float(row.input_cost)),
        ("friction", Value::Float(row.friction)),
        ("trend_bonus", Value::Float(row.trend_bonus)),
        ("profit", Value::Float(row.profit)),
        ("price_prior", Value::Bool(row.price_prior)),
        ("prior", Value::Bool(row.prior)),
    ])
}

/// The key and confidence columns every rates row repeats.
fn key_fields(estimate: &RateEstimate, key: &YieldKey) -> Vec<(&'static str, Value)> {
    vec![
        ("method", str_value(&key.method)),
        ("style", str_value(&key.style)),
        ("site", str_value(&key.site)),
        ("tool", str_value(&key.tool)),
        ("band", Value::Int(key.band)),
        ("runs", Value::Int(estimate.confidence.runs as i64)),
        ("prior", Value::Bool(estimate.prior)),
        ("read", str_value(&format!("{:?}", estimate.fallback))),
        ("age_secs", Value::Int(estimate.confidence.age_secs)),
        ("dispersion", Value::Float(estimate.confidence.dispersion)),
        ("failure_rate", Value::Float(estimate.confidence.failure_rate)),
        ("rejected", Value::Int(estimate.confidence.rejected as i64)),
    ]
}

/// Rows of the page, one estimate per bucket key (exact and roll-ups):
/// (flows: products and consumption, units, derived XP).
pub fn estimate_rows(snapshot: &Snapshot, now: i64) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut flows = Vec::new();
    let mut units = Vec::new();
    let mut xp = Vec::new();
    for key in snapshot.book.buckets.keys() {
        let estimate = match crate::rates_read::estimate_key(&snapshot.book, key, now) {
            Ok(estimate) => estimate,
            Err(error) => {
                let fields = vec![
                    ("method", str_value(&key.method)),
                    ("style", str_value(&key.style)),
                    ("site", str_value(&key.site)),
                    ("tool", str_value(&key.tool)),
                    ("band", Value::Int(key.band)),
                    ("error", str_value(&error.to_string())),
                ];
                flows.push(object(fields));
                continue;
            }
        };
        for (item, cause, rate) in &estimate.flows {
            let mut fields = key_fields(&estimate, key);
            fields.extend([("item", str_value(item)), ("cause", str_value(cause)), ("per_6000", Value::Float(*rate))]);
            flows.push(object(fields));
        }
        for (kind, subject, rate) in &estimate.units {
            let mut fields = key_fields(&estimate, key);
            fields.extend([("kind", str_value(kind)), ("subject", str_value(subject)), ("per_6000", Value::Float(*rate))]);
            units.push(object(fields));
        }
        for (skill, rate) in &estimate.xp {
            let mut fields = key_fields(&estimate, key);
            fields.extend([("skill", str_value(skill)), ("per_6000", Value::Float(*rate))]);
            xp.push(object(fields));
        }
    }
    (flows, units, xp)
}

/// Dashboard payload: opens the daemon store, serves the estimate of
/// every key (yields and consumption, units, derived XP, confidence,
/// prior or measured) and profitability, plus engaged counts per script counted from
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

/// Full rates payload for the dashboard: yield rows, XP rows,
/// profitability (priced through the store), the trading table and
/// engaged counts supplied by the caller (operator eyes only, never read
/// by bots).
pub fn rates_payload(store: &Store, snapshot: &Snapshot, engaged: BTreeMap<String, i64>) -> Value {
    let (yields, units, xp) = estimate_rows(snapshot, crate::rates_service::now_secs());
    let profits: Vec<Value> = profit_rows(store, snapshot).iter().map(profit_value).collect();
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
        ("profit", Value::Arr(profits)),
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
