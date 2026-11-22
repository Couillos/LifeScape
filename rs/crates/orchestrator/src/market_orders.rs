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
use lifescape_store::{OrderSide, OrderStatus};

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
        // The walk's live ceiling (a buy's limit, following the wallet) is
        // stored with the replacement: a lineage placed before schema 10
        // gains one at its first amend (read before the store lock).
        // Pressed (blocking need, urgent sale): an auction object serves it
        // at once after the amend (`pressed_auction`), like a placement.
        let pressed = row.motive == crate::market_lineage::NEED || crate::market_reprice::bounds_of(self, order_id).is_some_and(|bounds| bounds.pressed);
        let walked_ceiling = crate::market_reprice::bounds_of(self, order_id).filter(|bounds| !bounds.sell && bounds.ceiling < crate::market_reprice::UNSAID_CEILING).map(|bounds| bounds.ceiling);
        // Plan prix lot D: per unit unless the caller says `per_lot`.
        let walked_limit = crate::market_reprice::bounds_of(self, order_id).filter(|bounds| bounds.sell).map(|bounds| bounds.floor);
        let price = if request.get("per_lot").as_bool() == Some(true) { price } else { crate::market_units::to_lot(item, price) };
        if let Err(error) = check_price(price) {
            return Self::err(error.to_string());
        }
        // A raised bid is covered like a fresh one (user 2026-11-19, point
        // 1): the extra coins come out of the free coins, else refused.
        // `max_remaining` (review lot 4): the remainder cut to the need of
        // now, never raised; a raise with a cut is covered on the NEW
        // remainder (re-review lot 4, point 2).
        let cap = request.get("max_remaining").as_i64().filter(|cap| *cap >= 1);
        let kept = cap.map(|cap| cap.min(row.remaining)).unwrap_or(row.remaining);
        if row.side == lifescape_store::OrderSide::Buy && price > row.price {
            let (_, free) = self.free_coverage(&account, lifescape_store::OrderSide::Buy, item);
            let extra = crate::market_units::deal_coins(item, kept, price) - crate::market_units::deal_coins(item, row.remaining, row.price);
            let covered = if kept < row.remaining { extra <= free } else { crate::market_audit::affordable_raise(item, row.remaining, row.price, price, free) >= price };
            if !covered {
                return Self::err(format!("market rejected: buyer holds {free} free coins, the raise needs {extra}"));
            }
        }
        let clock = Self::now_clock();
        // 1. Book: the old order leaves; its true remainder rides along.
        let Some(old) = self.withdraw_from_book(item, order_id) else {
            return Self::err(format!("order {order_id} is not resting (filled or in flight)"));
        };
        let remaining = cap.map(|cap| cap.min(old.remaining)).unwrap_or(old.remaining);
        // 2. Carnet: the SAME row changes in place (schema 12, user
        // 2026-11-22: a reprice is not a cancellation): price, remaining,
        // limits and update clock; the journal `order_amends` keeps the
        // walk. Any refusal rolls the book back, so book and carnet never
        // diverge.
        let replacement = order_id;
        {
            let amended = self.lock_store().amend_order_in_place(&lifescape_store::OrderAmend { order_id, price, remaining: cap.map(|cap| cap.min(old.remaining)), ceiling: walked_ceiling, ask_limit: walked_limit, clock });
            if let Err(error) = amended {
                // Invalid: the book copy is intact, restore it. NotFound:
                // the carnet had closed it, the stale book copy stays gone.
                if matches!(error, lifescape_store::Error::NotFound(_)) {
                    return Self::err(format!("order {order_id} is closed ({error})"));
                }
                self.restore_to_book(old);
                return Self::err(error.to_string());
            }
        }
        // 3. Book: the replacement places like a fresh order and may
        // cross (same pre-match snapshot as `op_place`).
        let placed = {
            let mut guard = self.lock_engine();
            let engine: &mut crate::market_service::Engine = &mut guard;
            let kept_apart = crate::market_uncross::kept_apart(&engine.pair_cooldowns, crate::market_convene::wall_now());
            let book = engine.books.entry(item).or_default();
            let pre: HashMap<u64, Order> = book.orders().into_iter().map(|order| (order.id, order)).collect();
            let taker_seq = book.next_seq();
            book.place_guarded(replacement as u64, &account, item, side, remaining, price, &kept_apart).map(|(fills, _rests)| {
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
                crate::logln!("market amend: {account} order {order_id} withdrawn, amended order refused: {error}");
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
        if pressed {
            let served = self.pressed_auction(item, &row.motive, true);
            if !served.is_empty() {
                let mine = self.lock_store().open_deals_for_account(&account).unwrap_or_default();
                deal_ids.extend(mine.iter().filter(|deal| served.contains(&deal.deal_id)).map(|deal| Value::Int(deal.deal_id)));
            }
        }
        object(vec![
            ("ok", Value::Bool(true)),
            ("order", Value::Int(replacement)),
            ("deals", Value::Arr(deal_ids)),
        ])
    }

    /// `cancel {account, order, reason?}`: `reason` names the canceller
    /// (`gear_plan`, `audit`, `need_gone`…) for the measures of
    /// `orchestrator economy`; `amended` is reserved to `amend`.
    pub(crate) fn op_cancel(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let order_id = request.get("order").as_i64().unwrap_or(-1);
        let reason = request.get("reason").as_str().unwrap_or("").to_string();
        if reason == lifescape_store::AMENDED_REASON {
            return Self::err(format!("cancel reason '{reason}' is reserved to amend"));
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
        if self.withdraw_from_book(row.item_id, order_id).is_none() {
            return Self::err(format!("order {order_id} is not resting (filled or in flight)"));
        }
        let clock = Self::now_clock();
        self.lock_engine().quote_bounds.remove(&order_id);
        if let Err(error) = self.lock_store().cancel_order_because(order_id, clock, &reason) {
            // The carnet had closed it: the stale book copy is gone now.
            return Self::err(format!("order {order_id} is closed ({error})"));
        }
        object(vec![("ok", Value::Bool(true)), ("cancelled", Value::Int(order_id))])
    }

    /// True while `deal` is in flight (neither settled nor failed).
    pub(crate) fn deal_open(&self, deal: u64) -> bool {
        self.lock_engine()
            .deals
            .get(&deal)
            .is_some_and(|row| !matches!(row.status, lifescape_market::DealStatus::Settled | lifescape_market::DealStatus::Failed))
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
        self.cancel_open_because(account, item, side, "")
    }

    /// `cancel_open_for` recording why (`op_cancel` reasons).
    pub(crate) fn cancel_open_because(&self, account: &str, item: i64, side: OrderSide, reason: &str) -> usize {
        let mut closed = 0;
        for row in self.resting_rows(account, item, side) {
            let reply = self.op_cancel(&object(vec![("account", str_value(account)), ("order", Value::Int(row.order_id)), ("reason", str_value(reason))]));
            if reply.get("ok").as_bool() == Some(true) {
                closed += 1;
            }
        }
        closed
    }
}
