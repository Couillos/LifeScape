#![forbid(unsafe_code)]
//! Test support: drive every matched deal to `settled` through the real
//! state machine and the real `deal_report` op. Since 2026-11-13 a match
//! is a promise; executions, candles and prints exist only once the
//! handover settled in game.

use lifescape_json::{object, str_value, Value};
use lifescape_market::DealStatus;

use crate::market_service::MarketService;

pub(crate) fn settle_matched(service: &MarketService) -> usize {
    let waiting: Vec<(u64, String)> = service
        .lock_engine()
        .deals
        .iter()
        .filter(|(_, deal)| deal.status == DealStatus::Matched)
        .map(|(id, deal)| (*id, deal.buyer.clone()))
        .collect();
    for (id, buyer) in &waiting {
        {
            let mut engine = service.lock_engine();
            let deal = engine.deals.get_mut(id).expect("deal");
            for next in [DealStatus::Convened, DealStatus::BothArrived, DealStatus::Trading] {
                deal.transition(next, None).expect("walks to trading");
            }
        }
        let reply = service.handle(&object(vec![
            ("op", str_value("deal_report")),
            ("account", str_value(buyer)),
            ("deal", Value::Int(*id as i64)),
            ("outcome", str_value("settled")),
        ]));
        assert_eq!(reply.get("ok").as_bool(), Some(true), "settles: {reply:?}");
    }
    waiting.len()
}
