#![forbid(unsafe_code)]
//! Settled prints and their taker side (buy/sell volume split, user
//! 2026-11-13: "quand un mec achete il consomme des ordres de vente, et
//! inversement pour les vendeurs").
//!
//! The order that consumed the book is the taker (`Fill::taker`); its
//! side rides the deal row, then the execution written when the handover
//! settles, the candle split (`vb`/`vs`) and the live-tail print (`s`).
//! Rows settled before the split carry no side: `s` is null and their
//! quantity counts in the total volume only, never guessed.

use lifescape_json::{object, str_value, Value};
use lifescape_market::Side;
use lifescape_store::OrderSide;

use crate::market_service::{MarketService, Print};

/// Book side to the store's side (deal and execution taker column).
pub(crate) fn store_side(side: Side) -> OrderSide {
    match side {
        Side::Buy => OrderSide::Buy,
        Side::Sell => OrderSide::Sell,
    }
}

/// `"buy"`, `"sell"` or null (side unknown, pre-split row).
pub(crate) fn side_value(side: Option<OrderSide>) -> Value {
    side.map(|known| str_value(known.as_str())).unwrap_or(Value::Null)
}

/// One live-tail print `{item,q,p,t,s,buyer,seller}` (`op_trades`,
/// `op_tick`, `history`), makers flagged for the page.
pub(crate) fn print_value(print: &Print) -> Value {
    object(vec![
        ("item", Value::Int(print.item)),
        ("q", Value::Int(print.quantity)),
        ("p", Value::Int(print.price)),
        ("t", Value::Int(print.clock)),
        ("s", side_value(print.taker)),
        ("buyer", str_value(&print.buyer)),
        ("seller", str_value(&print.seller)),
        ("buyer_maker", Value::Bool(crate::market_facts::is_maker(&print.buyer))),
        ("seller_maker", Value::Bool(crate::market_facts::is_maker(&print.seller))),
    ])
}

/// Prints the live ring holds (`Engine::remember` keeps 200).
const RING_PRINTS: i64 = 200;

impl MarketService {
    /// Refills the live print ring (`trades`, `tick`, the WS live tail)
    /// from the execution log at daemon start. Live 2026-11-13: the ring
    /// lived in memory only, so after a restart `trades` listed the prints
    /// settled since (Bronze axe: 1) while `history` listed them all (8).
    pub(crate) fn reload_prints(&self) {
        // The settled deals (the executions' source) carry the parties.
        let deals = match self.lock_store().settled_deals(None, RING_PRINTS) {
            Ok(deals) => deals,
            Err(error) => {
                crate::logln!("market: print ring not reloaded ({error})");
                return;
            }
        };
        let mut engine = self.lock_engine();
        for deal in deals.into_iter().rev() {
            engine.remember(Print {
                item: deal.item_id,
                quantity: deal.quantity,
                price: deal.price,
                clock: deal.updated_clock,
                taker: deal.taker_side,
                buyer: deal.buyer,
                seller: deal.seller,
            });
        }
    }

    /// A settled handover becomes public: execution row (with the deal's
    /// taker side), candle repaint (`history` reads the candles cache,
    /// never executions) and the live-tail print. Live 2026-11-13:
    /// prints and candles were written at match time, so the 90 % of
    /// handovers that failed still showed as trades ("Last trades",
    /// volumes, candles).
    pub(crate) fn settle_print(&self, deal_id: i64, item: i64, qty: i64, price: i64, clock: i64) {
        let (taker, buyer, seller) = {
            let mut store = self.lock_store();
            let (taker, buyer, seller) = match store.get_deal(deal_id) {
                Ok(Some(deal)) => (deal.taker_side, deal.buyer, deal.seller),
                Ok(None) => (None, String::new(), String::new()),
                Err(error) => {
                    crate::logln!("market: deal {deal_id} taker side unreadable ({error}), printed as unknown");
                    (None, String::new(), String::new())
                }
            };
            let recorded = store.record_execution(&lifescape_store::NewExecution {
                item_id: item,
                quantity: qty,
                price,
                clock,
                server_tick: clock,
                taker_side: taker,
            });
            if let Err(error) = recorded {
                crate::logln!("market: settled execution not recorded ({error})");
                return;
            }
            // Every cached resolution (15 min, hour, day) in one pass.
            if let Err(error) = store.rebuild_all_candles(item) {
                crate::logln!("market: candles of item {item} not repainted ({error})");
            }
            (taker, buyer, seller)
        };
        let mut engine = self.lock_engine();
        engine.note_fill(item, price, qty);
        engine.remember(Print { item, quantity: qty, price, clock, taker, buyer, seller });
    }
}
