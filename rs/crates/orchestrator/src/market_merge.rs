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
    /// price into one, directly in book and carnet: the fragments were
    /// covered and inside the price band when placed, so the fold never
    /// runs them through the placement guards again (a band that moved
    /// since, or a stale ledger, used to refuse the re-place after every
    /// fragment was cancelled: the lots vanished). Returns the fragments
    /// merged away.
    pub(crate) fn consolidate_round(&self) -> usize {
        let resting: Vec<lifescape_store::Order> = self
            .lock_store()
            .resting_orders()
            .unwrap_or_default()
            .into_iter()
            .filter(|order| {
                order.remaining > 0
                    && matches!(order.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
            })
            .collect();
        let mut groups: BTreeMap<(String, i64, bool, i64), Vec<lifescape_store::Order>> = BTreeMap::new();
        for order in resting {
            let sell = order.side == lifescape_store::OrderSide::Sell;
            groups.entry((order.account.clone(), order.item_id, sell, order.price)).or_default().push(order);
        }
        let mut merged = 0usize;
        for ((account, item, sell, price), mut rows) in groups {
            if rows.len() < 2 {
                continue;
            }
            rows.sort_by_key(|row| (row.created_clock, row.order_id));
            let side = if sell { "sell" } else { "buy" };
            match self.fold_fragments(&account, item, sell, price, &rows) {
                Ok((folded, total)) => {
                    merged += folded.saturating_sub(1);
                    crate::logln!("market-merge: {account}: {folded} {side} order(s) on obj {item} at {price} merged into one of {total}");
                }
                Err(error) => crate::logln!("market-merge: {account}: {side} obj {item} at {price} not folded: {error}"),
            }
        }
        merged
    }

    /// Folds one group of fragments (same account, object, side, price;
    /// oldest first). Returns (fragments folded, merged remainder).
    ///
    /// Engine and store are never held together:
    /// 1. carnet: the merged row is created (open, at the oldest
    ///    fragment's clock so a reload keeps its rank);
    /// 2. book, one engine section: the fragments leave and the merged
    ///    order enters with their summed remainder and the oldest rank,
    ///    without matching (the fragments rested uncrossed at that very
    ///    price). When the book disagrees with the carnet read (a fill or
    ///    cancel raced the round), the fragments go back untouched and the
    ///    next round retries: the merged row is never resized afterwards,
    ///    where it could overwrite a concurrent fill's refresh;
    /// 3. carnet: the folded fragments close, or the merged row closes
    ///    when nothing folded.
    fn fold_fragments(&self, account: &str, item: i64, sell: bool, price: i64, rows: &[lifescape_store::Order]) -> Result<(usize, i64), String> {
        let clock = Self::now_clock();
        let planned: i64 = rows.iter().map(|row| row.remaining).sum();
        let oldest_clock = rows.first().map(|row| row.created_clock).ok_or("no fragment")?;
        let merged_id = self
            .lock_store()
            .place_order(&lifescape_store::NewOrder {
                account: account.to_string(),
                side: if sell { lifescape_store::OrderSide::Sell } else { lifescape_store::OrderSide::Buy },
                item_id: item,
                quantity: planned,
                price,
                dormant: false,
                clock: oldest_clock,
            })
            .map_err(|error| format!("merged row refused: {error}"))?;
        let taken = {
            let mut engine = self.lock_engine();
            let book = engine.books.entry(item).or_default();
            let taken: Vec<Order> = rows.iter().filter_map(|row| book.cancel(row.order_id as u64).ok()).collect();
            let total: i64 = taken.iter().map(|order| order.remaining).sum();
            match taken.iter().min_by_key(|order| order.seq).cloned() {
                Some(first) if taken.len() == rows.len() && total == planned => {
                    book.relist(Order { id: merged_id as u64, quantity: total, remaining: total, ..first });
                    engine.order_item.insert(merged_id as u64, item);
                    taken
                }
                _ => {
                    for order in taken {
                        book.relist(order);
                    }
                    Vec::new()
                }
            }
        };
        let store = self.lock_store();
        if taken.is_empty() {
            let _ = store.cancel_order(merged_id, clock);
            return Err("book and carnet disagree (a fill or cancel raced the round), retried next round".to_string());
        }
        for order in &taken {
            if let Err(error) = store.cancel_order(order.id as i64, clock) {
                crate::logln!("market-merge: fragment {} already closed in the carnet ({error})", order.id);
            }
        }
        drop(store);
        // The oldest fragment's repricing bounds carry over; the others go.
        let mut engine = self.lock_engine();
        let oldest = taken.iter().min_by_key(|order| order.seq).map(|order| order.id as i64);
        let bounds = oldest.and_then(|id| engine.quote_bounds.get(&id).cloned());
        for order in &taken {
            engine.quote_bounds.remove(&(order.id as i64));
        }
        if let Some(bounds) = bounds {
            engine.quote_bounds.insert(merged_id, bounds);
        }
        Ok((taken.len(), planned))
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
