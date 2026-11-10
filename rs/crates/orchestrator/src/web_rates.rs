//! Rates HTTP shaping (D44, D46): the trading-table JSON served beside
//! yields, XP and profit. Thin on purpose: aggregation lives in
//! `rates_service::trading`, this module owns the payload key contract
//! (same column keys as `api_shapes::TradingRow`).

#![forbid(unsafe_code)]

use lifescape_json::Value;
use lifescape_store::Store;

use super::trading::{trading_rows, trading_value};
use super::Snapshot;

/// Trading-table JSON rows for the dashboard payload.
pub fn trading_values(store: &Store, snapshot: &Snapshot) -> Vec<Value> {
    trading_rows(store, &snapshot.trades).iter().map(trading_value).collect()
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
        assert_eq!(row.get("stock_value").as_i64(), Some(300));
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
