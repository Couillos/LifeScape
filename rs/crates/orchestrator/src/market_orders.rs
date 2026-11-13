#![forbid(unsafe_code)]
//! Resting order lifecycle behind the market socket: `amend`, `cancel`
//! and the daemon-side withdrawal of an account's resting orders. Split
//! from `market_service` (file size), same `MarketService`.
//!
//! Book first, carnet second (review 2026-11-13). The in-memory book is
//! the authority on what still rests: an order is first taken out of it
//! under the engine lock alone, then the store rows move under the store
//! lock alone. Engine and store are never held together (live
//! 2026-11-12: nested locks taken in opposite orders froze the market).
//! Once out of the book, no concurrent `place` can fill the order, so
//! the carnet write cannot race a fill; an order the book no longer
//! holds is not "cancelled" (a fill took it, its row refresh writes the
//! final status). Final store rows never move again
//! (`Store::set_order_status`), so a late refresh cannot revive a
//! withdrawn order.

use std::collections::HashMap;

use lifescape_json::{object, str_value, Value};
use lifescape_market::{check_price, Order, Side};
use lifescape_store::{NewOrder, OrderSide, OrderStatus};

use crate::market_service::MarketService;

fn book_side(side: OrderSide) -> Side {
    match side {
        OrderSide::Buy => Side::Buy,
        OrderSide::Sell => Side::Sell,
    }
}

impl MarketService {
    /// Takes one order out of its book. `None` when the book no longer
    /// holds it (filled, in flight, or never rested).
    fn withdraw_from_book(&self, item: i64, order_id: i64) -> Option<Order> {
        let mut engine = self.lock_engine();
        engine.books.get_mut(&item).and_then(|book| book.cancel(order_id as u64).ok())
    }

    /// Puts a withdrawn order back with its original rank (rollback).
    fn restore_to_book(&self, order: Order) {
        let mut engine = self.lock_engine();
        engine.books.entry(order.item).or_default().relist(order);
    }

    pub(crate) fn op_amend(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let order_id = request.get("order").as_i64().unwrap_or(-1);
        let price = request.get("price").as_i64().unwrap_or(0);
        if order_id < 1 {
            return Self::err(format!("bad order {order_id}"));
        }
        if price < 1 {
            return Self::err(format!("bad amended price {price}"));
        }
        let stored = self.lock_store().get_order(order_id).unwrap_or(None);
        let Some(row) = stored else {
            return Self::err(format!("unknown order {order_id}"));
        };
        if row.account != account {
            return Self::err("order belongs to another account".to_string());
        }
        if !matches!(row.status, OrderStatus::Open | OrderStatus::Matched) {
            return Self::err(format!("order {order_id} is closed"));
        }
        let item = row.item_id;
        let side = book_side(row.side);
        if let Err(error) = check_price(side, price, self.guard_anchor(item)) {
            return Self::err(error.to_string());
        }
        let clock = Self::now_clock();
        // 1. Book: the old order leaves; its true remainder rides along.
        let Some(old) = self.withdraw_from_book(item, order_id) else {
            return Self::err(format!("order {order_id} is not resting (filled or in flight)"));
        };
        // 2. Carnet: replacement row first, then the old row closes. Any
        // refusal rolls the book back, so book and carnet never diverge.
        let replacement = {
            let store = self.lock_store();
            let placed = store.place_order(&NewOrder {
                account: account.clone(),
                side: row.side,
                item_id: item,
                quantity: old.remaining,
                price,
                dormant: false,
                clock,
            });
            match placed {
                Ok(id) => match store.cancel_order(order_id, clock) {
                    Ok(()) => Ok(id),
                    Err(error) => {
                        // The carnet had closed it: the book copy was stale.
                        let _ = store.cancel_order(id, clock);
                        Err((false, format!("order {order_id} is closed ({error})")))
                    }
                },
                Err(error) => Err((true, error.to_string())),
            }
        };
        let replacement = match replacement {
            Ok(id) => id,
            Err((restore, error)) => {
                if restore {
                    self.restore_to_book(old);
                }
                return Self::err(error);
            }
        };
        {
            // Repricing bounds follow the order to its new id.
            let mut engine = self.lock_engine();
            if let Some(bounds) = engine.quote_bounds.remove(&order_id) {
                engine.quote_bounds.insert(replacement, bounds);
            }
        }
        // 3. Book: the replacement places like a fresh order and may
        // cross (same pre-match snapshot as `op_place`).
        let placed = {
            let mut engine = self.lock_engine();
            let book = engine.books.entry(item).or_default();
            let pre: HashMap<u64, Order> = book.orders().into_iter().map(|order| (order.id, order)).collect();
            let taker_seq = book.next_seq();
            book.place(replacement as u64, &account, item, side, old.remaining, price).map(|(fills, _rests)| {
                fills
                    .into_iter()
                    .map(|fill| {
                        let rows = crate::market_convene::relist_for_fill(&pre, taker_seq, side, &account, price, &fill);
                        (fill, rows)
                    })
                    .collect::<Vec<(lifescape_market::Fill, Vec<Order>)>>()
            })
        };
        let matched = match placed {
            Ok(matched) => matched,
            Err(error) => {
                // Validated above: unreachable in practice. The lots are
                // withdrawn from both book and carnet, never half-listed.
                let _ = self.lock_store().cancel_order(replacement, clock);
                crate::logln!("market amend: {account} order {order_id} withdrawn, replacement refused: {error}");
                return Self::err(error.to_string());
            }
        };
        let fills: Vec<lifescape_market::Fill> = matched.iter().map(|(fill, _)| fill.clone()).collect();
        let matched = crate::market_merge::merge_fills(matched);
        self.lock_engine().order_item.insert(replacement as u64, item);
        let mut deal_ids = Vec::new();
        for (fill, rows) in &matched {
            match self.record_fill(fill, rows.clone(), clock) {
                Ok(deal_id) => deal_ids.push(Value::Int(deal_id)),
                Err(error) => return Self::err(error),
            }
        }
        self.refresh_order_rows(replacement as u64, &fills, clock);
        object(vec![
            ("ok", Value::Bool(true)),
            ("order", Value::Int(replacement)),
            ("cancelled", Value::Int(order_id)),
            ("deals", Value::Arr(deal_ids)),
        ])
    }

    pub(crate) fn op_cancel(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let order_id = request.get("order").as_i64().unwrap_or(-1);
        let stored = self.lock_store().get_order(order_id).unwrap_or(None);
        let Some(row) = stored else {
            return Self::err(format!("unknown order {order_id}"));
        };
        if row.account != account {
            return Self::err("order belongs to another account".to_string());
        }
        if !matches!(row.status, OrderStatus::Open | OrderStatus::Matched) {
            return Self::err(format!("order {order_id} is closed"));
        }
        if self.withdraw_from_book(row.item_id, order_id).is_none() {
            return Self::err(format!("order {order_id} is not resting (filled or in flight)"));
        }
        let clock = Self::now_clock();
        self.lock_engine().quote_bounds.remove(&order_id);
        if let Err(error) = self.lock_store().cancel_order(order_id, clock) {
            // The carnet had closed it: the stale book copy is gone now.
            return Self::err(format!("order {order_id} is closed ({error})"));
        }
        object(vec![("ok", Value::Bool(true)), ("cancelled", Value::Int(order_id))])
    }

    /// Resting carnet rows of `account` on `item` and `side`: open ones
    /// and the resting remainder of partly filled (`matched`) ones.
    pub(crate) fn resting_rows(&self, account: &str, item: i64, side: OrderSide) -> Vec<lifescape_store::Order> {
        self.lock_store()
            .account_orders(account)
            .unwrap_or_default()
            .into_iter()
            .filter(|row| {
                row.item_id == item
                    && row.side == side
                    && row.remaining > 0
                    && matches!(row.status, OrderStatus::Open | OrderStatus::Matched)
            })
            .collect()
    }

    /// Cancels every resting order of `account` on `item` and `side`,
    /// partly filled remainders included (daemon-side, no socket): the
    /// brain calls it when a fill wait expires, so a retry never stacks a
    /// second resting order beside the stale one; a Short handover calls
    /// it on the defaulter's phantom lots. Returns how many it closed.
    pub(crate) fn cancel_open_for(&self, account: &str, item: i64, side: OrderSide) -> usize {
        let mut closed = 0;
        for row in self.resting_rows(account, item, side) {
            let reply = self.op_cancel(&object(vec![("account", str_value(account)), ("order", Value::Int(row.order_id))]));
            if reply.get("ok").as_bool() == Some(true) {
                closed += 1;
            }
        }
        closed
    }
}
