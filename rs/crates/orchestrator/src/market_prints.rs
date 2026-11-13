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

/// One live-tail print `{item,q,p,t,s}` (`op_trades`, `op_tick`).
pub(crate) fn print_value(print: &Print) -> Value {
    object(vec![
        ("item", Value::Int(print.item)),
        ("q", Value::Int(print.quantity)),
        ("p", Value::Int(print.price)),
        ("t", Value::Int(print.clock)),
        ("s", side_value(print.taker)),
    ])
}

impl MarketService {
    /// A settled handover becomes public: execution row (with the deal's
    /// taker side), candle repaint (`history` reads the candles cache,
    /// never executions) and the live-tail print. Live 2026-11-13:
    /// prints and candles were written at match time, so the 90 % of
    /// handovers that failed still showed as trades ("Last trades",
    /// volumes, candles).
    pub(crate) fn settle_print(&self, deal_id: i64, item: i64, qty: i64, price: i64, clock: i64) {
        let taker = {
            let mut store = self.lock_store();
            let taker = match store.get_deal(deal_id) {
                Ok(found) => found.and_then(|deal| deal.taker_side),
                Err(error) => {
                    crate::logln!("market: deal {deal_id} taker side unreadable ({error}), printed as unknown");
                    None
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
            let _ = store.rebuild_candles(item, lifescape_store::YieldCandleResolution::Hour1);
            let _ = store.rebuild_candles(item, lifescape_store::YieldCandleResolution::Day1);
            taker
        };
        self.lock_engine().remember(Print { item, quantity: qty, price, clock, taker });
    }
}
