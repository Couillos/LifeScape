#![forbid(unsafe_code)]
//! Deal ends (split from `market_convene`, rule 4): convene release,
//! deadline sweep and absent-party failure. Every path out of a deal
//! frees what it held and refunds the ledger through
//! `refund_and_relist`.

use lifescape_json::{object, str_value, Value};
use lifescape_market::{DealStatus, FailReason};

use crate::market_service::MarketService;

impl MarketService {
    /// Frees convene state (tiles, deadline, account holds) at a terminal
    /// deal, whatever the outcome. An account stays convened while any
    /// other deal of its convocation is still in flight: freeing it early
    /// let a new convocation overwrite the pending pose on the worker, the
    /// overwritten deals were never delivered and the honest partner got
    /// `no_show` (finding 4).
    pub(crate) fn release_convene(&self, deal_id: u64) {
        let mut engine = self.lock_engine();
        if let Some(tiles) = engine.deal_tiles.remove(&deal_id) {
            engine.ring_busy.remove(&tiles.0);
            engine.ring_busy.remove(&tiles.1);
        }
        engine.deal_sweep_wall.remove(&deal_id);
        engine.pending_since_wall.remove(&deal_id);
        engine.arrived.remove(&deal_id);
        let parties = engine.deals.get(&deal_id).map(|deal| (deal.buyer.clone(), deal.seller.clone()));
        if let Some((buyer, seller)) = parties {
            for account in [buyer, seller] {
                let in_flight = engine.deals.iter().any(|(other, deal)| {
                    *other != deal_id
                        && (deal.buyer == account || deal.seller == account)
                        && matches!(deal.status, DealStatus::Convened | DealStatus::BothArrived | DealStatus::Trading)
                });
                if !in_flight {
                    engine.convened_accounts.remove(&account);
                }
            }
        }
    }

    /// Expires convened deals past their deadline. Arrival-gated like the
    /// live reports: the party that showed up relists with priority and
    /// keeps reliability, the absent loses both. Without nav every
    /// expiry is `nav_down`, neutral, both relist: nobody travels blind.
    /// A deal stuck in `trading` (worker crashed or disconnected after its
    /// report, finding 2) expires `timeout` too, neutral.
    pub(crate) fn op_sweep(&self, request: &Value) -> Value {
        let now_wall = request.get("now_wall").as_f64().unwrap_or(0.0);
        let nav_up = request.get("nav_up").as_bool().unwrap_or(true);
        let expired: Vec<u64> = {
            let engine = self.lock_engine();
            let mut expired: Vec<u64> = engine
                .deal_sweep_wall
                .iter()
                .filter(|(id, wall)| {
                    **wall < now_wall
                        && engine.deals.get(*id).is_some_and(|deal| {
                            matches!(deal.status, DealStatus::Convened | DealStatus::BothArrived | DealStatus::Trading)
                        })
                })
                .map(|(id, _)| *id)
                .collect();
            expired.sort_unstable();
            expired
        };
        let mut failed = Vec::new();
        for deal_id in expired {
            let (buyer, seller, item, qty, price, status, marks) = {
                let engine = self.lock_engine();
                let Some(deal) = engine.deals.get(&deal_id) else {
                    continue;
                };
                (
                    deal.buyer.clone(),
                    deal.seller.clone(),
                    deal.item,
                    deal.quantity,
                    deal.price,
                    deal.status,
                    engine.arrived.get(&deal_id).cloned().unwrap_or((false, false)),
                )
            };
            let reason = if !nav_up {
                FailReason::NavDown
            } else if matches!(status, DealStatus::BothArrived | DealStatus::Trading) {
                FailReason::Timeout
            } else {
                FailReason::NoShow
            };
            {
                let mut engine = self.lock_engine();
                let Some(deal) = engine.deals.get_mut(&deal_id) else {
                    continue;
                };
                if deal.transition(DealStatus::Failed, Some(reason)).is_err() {
                    continue;
                }
            }
            self.lock_store()
                .set_deal_status(deal_id as i64, lifescape_store::DealStatus::Failed, reason.as_str(), Self::now_clock())
                .ok();
            let (relist_buyer, relist_seller, reliability) = match reason {
                FailReason::NoShow => (marks.0, marks.1, (arrival_score(marks.0), arrival_score(marks.1))),
                FailReason::Timeout => (true, true, (None, None)),
                _ => (true, true, (None, None)),
            };
            self.refund_and_relist(deal_id, &buyer, &seller, item, qty, price, relist_buyer, relist_seller, reliability);
            failed.push(Value::Int(deal_id as i64));
        }
        object(vec![("ok", Value::Bool(true)), ("failed", Value::Arr(failed))])
    }

    /// Fails every matched deal of an account that cannot be brought
    /// online (open refused or failed, or still absent past the grace):
    /// `no_show`, the honest side relists with its priority and keeps its
    /// reliability, the absent side loses both, and every order it still
    /// rests is cancelled (it would match and fail again forever). Same
    /// shape as the `short` path. Returns the failed deal ids.
    pub(crate) fn fail_absent_party(&self, account: &str) -> Vec<i64> {
        let mut waiting: Vec<u64> = self
            .lock_engine()
            .deals
            .iter()
            .filter(|(_, deal)| deal.status == DealStatus::Matched && (deal.buyer == account || deal.seller == account))
            .map(|(id, _)| *id)
            .collect();
        waiting.sort_unstable();
        let mut failed = Vec::new();
        for deal_id in waiting {
            let terms = {
                let mut engine = self.lock_engine();
                let Some(deal) = engine.deals.get_mut(&deal_id) else {
                    continue;
                };
                if deal.transition(DealStatus::Failed, Some(FailReason::NoShow)).is_err() {
                    continue;
                }
                (deal.buyer.clone(), deal.seller.clone(), deal.item, deal.quantity, deal.price)
            };
            let (buyer, seller, item, qty, price) = terms;
            self.lock_store()
                .set_deal_status(deal_id as i64, lifescape_store::DealStatus::Failed, FailReason::NoShow.as_str(), Self::now_clock())
                .ok();
            let buyer_present = buyer != account;
            let seller_present = seller != account;
            self.refund_and_relist(
                deal_id,
                &buyer,
                &seller,
                item,
                qty,
                price,
                buyer_present,
                seller_present,
                (arrival_score(buyer_present), arrival_score(seller_present)),
            );
            failed.push(deal_id as i64);
        }
        let rows = self.lock_store().account_orders(account).unwrap_or_default();
        let mut cancelled = 0;
        for row in rows.iter().filter(|row| {
            row.status == lifescape_store::OrderStatus::Open
                || (row.status == lifescape_store::OrderStatus::Matched && row.remaining > 0)
        }) {
            let reply = self.handle(&object(vec![
                ("op", str_value("cancel")),
                ("account", str_value(account)),
                ("order", Value::Int(row.order_id)),
            ]));
            if reply.get("ok").as_bool() == Some(true) {
                cancelled += 1;
            }
        }
        crate::logln!("market: {account} absent, deals {failed:?} failed no_show, {cancelled} order(s) cancelled");
        failed
    }
}

/// Reliability verdict for one side of a no-show: showing up counts,
/// standing the partner up does not.
pub(crate) fn arrival_score(arrived: bool) -> Option<bool> {
    Some(arrived)
}
