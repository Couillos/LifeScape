#![forbid(unsafe_code)]
//! Demo seeding for the market page: fixed-seed synthetic fills, resting
//! depth, candles and trends, so the front paints real shapes with zero
//! bots connected. Runs once, when the store holds no executions yet.

use lifescape_market::{compute_trend, Book, Side, TrendInput};
use lifescape_store::{NewOrder, OrderSide, Trend, YieldCandleResolution};

use super::market_service::MarketService;

/// One catalogue row (F2P tradeable item from gamedata).
#[derive(Clone, Debug)]
pub struct CatalogueItem {
    pub id: i64,
    pub name: String,
    pub shop_price: i64,
    pub alch_value: i64,
}

/// F2P tradeable catalogue straight from the dumps (members and
/// untradeable rows never reach the market).
pub fn catalogue() -> Vec<CatalogueItem> {
    lifescape_gamedata::f2p::f2p_tradeable_objs()
        .iter()
        .map(|obj| CatalogueItem {
            id: obj.obj_id as i64,
            name: obj.display.or(obj.name).unwrap_or("unknown").to_string(),
            shop_price: obj.price.max(0),
            alch_value: obj.alch_high.max(0),
        })
        .collect()
}

/// Demo history: fixed-seed synthetic agents per sampled item, so the
/// market page paints real shapes with zero bots connected. Runs once,
/// when the store holds no executions yet.
pub fn seed_demo_if_empty(service: &MarketService) -> Result<(), String> {
    {
        let store = service.lock_store();
        let catalogue_len = store.list_items().map_err(|error| error.to_string())?.len();
        if catalogue_len == 0 {
            return Err("empty catalogue".to_string());
        }
        // Any execution anywhere means a real or seeded market: stop.
        let mut any = false;
        for row in store.list_items().map_err(|error| error.to_string())?.iter().take(40) {
            if !store.executions_for_item(row.item_id, 0, i64::MAX).map_err(|error| error.to_string())?.is_empty() {
                any = true;
                break;
            }
        }
        if any {
            return Ok(());
        }
    }
    let picks: Vec<CatalogueItem> = {
        let engine = service.lock_engine();
        let mut ordered = engine.catalogue.clone();
        ordered.sort_by_key(|row| row.shop_price);
        let step = (ordered.len() / 24).max(1);
        ordered.into_iter().step_by(step).take(24).collect()
    };
    let clock = MarketService::now_clock();
    for (index, row) in picks.iter().enumerate() {
        let anchor = row.shop_price.max(2);
        let mut agents = lifescape_market::sim::scenario_agents(9000 + index as u64, 8, 8, anchor, (anchor / 4).max(2));
        let (report, path) =
            lifescape_market::sim::run_scenario(row.id, &mut agents, 220, 1, 60).map_err(|error| error.to_string())?;
        let _ = report;
        // Spread the prints over the last 36 hours so the 24h view
        // and the hourly candles both have shape; the tail stays
        // live (last print = now) for the WS tick.
        let span = 36 * 3600i64;
        let per = if path.is_empty() { 0 } else { span / path.len().max(1) as i64 };
        {
            let store = service.lock_store();
            for (position, price) in path.iter().enumerate() {
                let at = clock - span + position as i64 * per.max(1);
                store
                    .record_execution(&lifescape_store::NewExecution {
                        item_id: row.id,
                        quantity: 5 + (position % 7) as i64,
                        price: (*price).max(1),
                        clock: at,
                        server_tick: at,
                    })
                    .map_err(|error| error.to_string())?;
            }
        }
        // Resting depth around the last print: bids below, asks above.
        let last = path.last().cloned().unwrap_or(anchor).max(1);
        {
            let mut engine = service.lock_engine();
            engine.ledger.credit_coins("demo-merchant", 10_000_000);
            engine.ledger.credit_stock("demo-merchant", row.id, 100_000);
        }
        for level in 1..=5 {
            let buy_price = (last - level).max(1);
            let sell_price = last + level;
            for (side, price) in [(OrderSide::Buy, buy_price), (OrderSide::Sell, sell_price)] {
                let store_id = service
                    .lock_store()
                    .place_order(&NewOrder {
                        account: "demo-merchant".to_string(),
                        side,
                        item_id: row.id,
                        quantity: 20,
                        price,
                        dormant: false,
                        clock,
                    })
                    .map_err(|error| error.to_string())?;
                let market_side = if side == OrderSide::Buy { Side::Buy } else { Side::Sell };
                let mut engine = service.lock_engine();
                let book = engine.books.entry(row.id).or_insert_with(Book::new);
                // Resting quotes never cross by construction.
                let _ = book.place(store_id as u64, "demo-merchant", row.id, market_side, 20, price);
                engine.order_item.insert(store_id as u64, row.id);
            }
        }
        // Rebuild both candle resolutions from the seeded executions,
        // then score the public trend from book depth plus prints.
        service.rebuild_candles(row.id, YieldCandleResolution::Hour1)?;
        service.rebuild_candles(row.id, YieldCandleResolution::Day1)?;
        let (buy_depth, sell_depth) = {
            let engine = service.lock_engine();
            match engine.books.get(&row.id) {
                Some(book) => (
                    book.depth(Side::Buy, 10).iter().map(|level| level.1).sum(),
                    book.depth(Side::Sell, 10).iter().map(|level| level.1).sum(),
                ),
                None => (0, 0),
            }
        };
        let day = service.lock_store().executions_for_item(row.id, clock - 86400, clock + 1).map_err(|error| error.to_string())?;
        let scores = compute_trend(&TrendInput {
            buy_depth,
            sell_depth,
            first_price: day.first().map(|exec| exec.price),
            last_price: day.last().map(|exec| exec.price),
            previous_score: 0.0,
        });
        service.lock_store()
            .upsert_trend(&Trend {
                item_id: row.id,
                period: "24h".to_string(),
                shortage: scores.shortage,
                momentum: scores.momentum,
                score: scores.score,
                computed_clock: clock,
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
