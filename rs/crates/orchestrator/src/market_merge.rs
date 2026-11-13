#![forbid(unsafe_code)]
//! One counterparty, one price, one deal (user, 2026-11-13: "bbkjz2vk
//! achete des raw shrimps a fiona_park en plein d'ordres et au meme prix,
//! pourquoi on regroupe pas ?").
//!
//! Live: fiona_park held a dozen 1–6 unit asks at 1 coin, all born in the
//! same second from failed-deal relists; one bid crossed them all and the
//! book made nine deals, each failure relisting its own fragment again.
//! - fills of one placement with the same buyer, seller and price merge
//!   into one deal, their relist rows merged per account, side and price;
//! - the audit merges an account's resting orders on one object, side and
//!   price into one order (the oldest bounds carry over).

use std::collections::BTreeMap;

use lifescape_json::{object, str_value, Value};
use lifescape_market::{Fill, Order};

use crate::market_service::MarketService;

/// Merges fills sharing buyer, seller and price (first fill's ids kept),
/// in first-seen order; relist rows of a merged deal fold per account,
/// side and price (quantities summed, the earliest `seq` kept).
pub fn merge_fills(matched: Vec<(Fill, Vec<Order>)>) -> Vec<(Fill, Vec<Order>)> {
    let mut merged: Vec<(Fill, Vec<Order>)> = Vec::new();
    for (fill, rows) in matched {
        if let Some((kept, kept_rows)) = merged
            .iter_mut()
            .find(|(kept, _)| kept.buyer == fill.buyer && kept.seller == fill.seller && kept.price == fill.price && kept.item == fill.item)
        {
            kept.quantity += fill.quantity;
            kept_rows.extend(rows);
        } else {
            merged.push((fill, rows));
        }
    }
    for (_, rows) in &mut merged {
        *rows = fold_rows(std::mem::take(rows));
    }
    merged
}

fn fold_rows(rows: Vec<Order>) -> Vec<Order> {
    let mut folded: Vec<Order> = Vec::new();
    for row in rows {
        if let Some(kept) = folded.iter_mut().find(|kept| kept.account == row.account && kept.side == row.side && kept.price == row.price && kept.item == row.item) {
            kept.quantity += row.quantity;
            kept.remaining += row.remaining;
            kept.seq = kept.seq.min(row.seq);
        } else {
            folded.push(row);
        }
    }
    folded
}

impl MarketService {
    /// Merges every account's resting orders that share object, side and
    /// price into one: cancel the fragments, place their summed remainder
    /// once with the oldest fragment's repricing bounds. Returns the
    /// fragments merged away.
    pub(crate) fn consolidate_round(&self) -> usize {
        let open: Vec<lifescape_store::Order> = self
            .lock_store()
            .resting_orders()
            .unwrap_or_default()
            .into_iter()
            .filter(|order| order.status == lifescape_store::OrderStatus::Open && order.remaining > 0)
            .collect();
        let mut groups: BTreeMap<(String, i64, bool, i64), Vec<lifescape_store::Order>> = BTreeMap::new();
        for order in open {
            let sell = order.side == lifescape_store::OrderSide::Sell;
            groups.entry((order.account.clone(), order.item_id, sell, order.price)).or_default().push(order);
        }
        let mut merged = 0usize;
        for ((account, item, sell, price), mut rows) in groups {
            if rows.len() < 2 {
                continue;
            }
            rows.sort_by_key(|row| (row.created_clock, row.order_id));
            let total: i64 = rows.iter().map(|row| row.remaining).sum();
            let bounds = crate::market_reprice::bounds_of(rows[0].order_id);
            let mut cancelled = 0usize;
            for row in &rows {
                let reply = self.handle(&object(vec![
                    ("op", str_value("cancel")),
                    ("account", str_value(&account)),
                    ("order", Value::Int(row.order_id)),
                ]));
                if reply.get("ok").as_bool() == Some(true) {
                    cancelled += 1;
                }
            }
            if cancelled == 0 {
                continue;
            }
            let replaced: i64 = rows.iter().take(cancelled).map(|row| row.remaining).sum::<i64>().min(total);
            let mut fields = vec![
                ("op", str_value("place")),
                ("account", str_value(&account)),
                ("side", str_value(if sell { "sell" } else { "buy" })),
                ("item", Value::Int(item)),
                ("qty", Value::Int(replaced)),
                ("price", Value::Int(price)),
            ];
            if let Some(bounds) = bounds {
                if sell {
                    fields.push(("floor", Value::Int(bounds.floor)));
                } else {
                    fields.push(("ceiling", Value::Int(bounds.ceiling)));
                }
                fields.push(("patience", Value::Int(bounds.patience_ticks as i64)));
            }
            let reply = self.handle(&object(fields));
            if reply.get("ok").as_bool() == Some(true) {
                merged += cancelled.saturating_sub(1);
                crate::logln!("market-merge: {account}: {cancelled} {} order(s) on obj {item} at {price} merged into one of {replaced}", if sell { "sell" } else { "buy" });
            } else {
                crate::logln!("market-merge: {account}: re-place of {replaced} obj {item} refused: {:?}", reply.get("error").as_str());
            }
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::merge_fills;
    use lifescape_market::{Fill, Order, Side};

    fn fill(sell_id: u64, seller: &str, qty: i64, price: i64) -> Fill {
        Fill { buy_id: 1, sell_id, buyer: "bbkjz2vk".to_string(), seller: seller.to_string(), item: 317, quantity: qty, price }
    }

    fn row(account: &str, side: Side, qty: i64, price: i64, seq: u64) -> Order {
        Order { id: 0, account: account.to_string(), item: 317, side, quantity: qty, remaining: qty, price, seq }
    }

    /// Live 2026-11-13 (deals 827–835): one bid crossed nine of
    /// fiona_park's 1-coin asks; it is one deal of the summed quantity,
    /// relisting one row per party on failure.
    #[test]
    fn same_counterparty_same_price_is_one_deal() {
        let matched = vec![
            (fill(10, "fiona_park", 2, 1), vec![row("fiona_park", Side::Sell, 2, 1, 5), row("bbkjz2vk", Side::Buy, 2, 1, 9)]),
            (fill(11, "fiona_park", 13, 1), vec![row("fiona_park", Side::Sell, 13, 1, 3), row("bbkjz2vk", Side::Buy, 13, 1, 9)]),
            (fill(12, "fiona_park", 4, 2), vec![row("fiona_park", Side::Sell, 4, 2, 7), row("bbkjz2vk", Side::Buy, 4, 1, 9)]),
            (fill(13, "harry_page", 1, 1), vec![row("harry_page", Side::Sell, 1, 1, 8), row("bbkjz2vk", Side::Buy, 1, 1, 9)]),
        ];
        let merged = merge_fills(matched);
        assert_eq!(merged.len(), 3, "fiona@1 merged; fiona@2 and harry@1 apart");
        assert_eq!(merged[0].0.quantity, 15);
        let rows = &merged[0].1;
        assert_eq!(rows.len(), 2, "one relist row per party: {rows:?}");
        let seller = rows.iter().find(|row| row.account == "fiona_park").expect("seller row");
        assert_eq!((seller.remaining, seller.seq), (15, 3), "summed, earliest priority");
        assert_eq!(merged[1].0.price, 2);
        assert_eq!(merged[2].0.seller, "harry_page");
    }
}
