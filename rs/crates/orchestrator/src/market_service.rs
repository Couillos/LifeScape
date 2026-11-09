#![forbid(unsafe_code)]
//! Daemon-side market: catalogue, matching engine, ledger and socket.
//!
//! One `run/market.sock` (JSON-lines, one request one reply, like
//! `pathfinding.sock`): `quote booking history place amend cancel
//! my_orders my_deals deal_arrived deal_report fund seed_demo tick
//! stats`. Persistence goes through `lifescape-store`, opened here by
//! the daemon alone; workers never touch the database file.
//!
//! Balances live in a small in-memory ledger: connected bots will feed
//! real holdings in a later lot (the check itself already runs here, so
//! the rule cannot be bypassed when that wiring lands). Deals settle the
//! ledger at match time; a later `failed` report refunds the transfer
//! and both sides keep their reliability counters.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{check_coverage, check_price, reference_price};
use crate::market_seed::{catalogue, CatalogueItem};
use lifescape_market::{Book, Deal, DealStatus, FailReason, Side};
use lifescape_store::{NewDeal, NewOrder, OrderSide, OrderStatus, Store, YieldCandleResolution};

/// Socket the workers dial (beside `session-<account>.sock`).
pub fn socket_path() -> PathBuf {
    crate::paths::run_dir().join("market.sock")
}

/// Database file, opened by the daemon alone.
pub fn db_path() -> PathBuf {
    crate::paths::run_dir().join("lifescape.db")
}

/// Varrock fountain basin centre (map survey, rev 274): all handovers
/// happen on the ring around it.
pub const FOUNTAIN_X: i64 = 3213;
pub const FOUNTAIN_Y: i64 = 3429;
/// Ring of handover tiles around the basin, second ring past eight.
const RING: [(i64, i64); 8] =
    [(3211, 3427), (3212, 3427), (3214, 3427), (3215, 3427), (3211, 3431), (3212, 3431), (3214, 3431), (3215, 3431)];

/// In-memory ledger: coins and item units per account. Later lots feed
/// live holdings for connected bots; the coverage check already runs
/// against this table either way.
#[derive(Default)]
pub(crate) struct Ledger {
    pub(crate) coins: HashMap<String, i64>,
    pub(crate) stock: HashMap<(String, i64), i64>,
}

impl Ledger {
    pub(crate) fn coins_of(&self, account: &str) -> i64 {
        self.coins.get(account).cloned().unwrap_or(0)
    }

    pub(crate) fn stock_of(&self, account: &str, item: i64) -> i64 {
        self.stock.get(&(account.to_string(), item)).cloned().unwrap_or(0)
    }

    pub(crate) fn credit_coins(&mut self, account: &str, amount: i64) {
        *self.coins.entry(account.to_string()).or_insert(0) += amount;
    }

    pub(crate) fn credit_stock(&mut self, account: &str, item: i64, qty: i64) {
        *self.stock.entry((account.to_string(), item)).or_insert(0) += qty;
    }
}

/// Recent print kept for the WS live tail (no cross-item store query).
#[derive(Clone, Debug)]
pub(crate) struct Print {
    pub(crate) item: i64,
    pub(crate) quantity: i64,
    pub(crate) price: i64,
    pub(crate) clock: i64,
}

pub(crate) struct Engine {
    pub(crate) catalogue: Vec<CatalogueItem>,
    pub(crate) books: HashMap<i64, Book>,
    pub(crate) order_item: HashMap<u64, i64>,
    pub(crate) deals: HashMap<u64, Deal>,
    pub(crate) arrived: HashMap<u64, (bool, bool)>,
    pub(crate) ledger: Ledger,
    pub(crate) recent: VecDeque<Print>,
    next_seq: u64,
    placed: u64,
    matched: u64,
}

impl Engine {
    pub(crate) fn new(catalogue: Vec<CatalogueItem>) -> Self {
        Engine {
            catalogue,
            books: HashMap::new(),
            order_item: HashMap::new(),
            deals: HashMap::new(),
            arrived: HashMap::new(),
            ledger: Ledger::default(),
            recent: VecDeque::new(),
            next_seq: 1,
            placed: 0,
            matched: 0,
        }
    }

    pub(crate) fn find_item(&self, item: i64) -> Option<&CatalogueItem> {
        self.catalogue.iter().find(|entry| entry.id == item)
    }

    pub(crate) fn remember(&mut self, print: Print) {
        self.recent.push_back(print);
        while self.recent.len() > 200 {
            self.recent.pop_front();
        }
        self.next_seq += 1;
    }
}

/// The daemon market: store plus engine behind one lock each.
pub struct MarketService {
    store: Mutex<Store>,
    engine: Mutex<Engine>,
}

impl MarketService {
    pub(crate) fn rebuild_candles(&self, item: i64, resolution: lifescape_store::YieldCandleResolution) -> Result<(), String> {
        self.lock_store()
            .rebuild_candles(item, resolution)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn open(db: &std::path::Path, catalogue: Vec<CatalogueItem>) -> Result<Self, String> {
        let store = Store::open(db).map_err(|error| error.to_string())?;
        for item in &catalogue {
            store
                .upsert_item(&lifescape_store::Item {
                    item_id: item.id,
                    name: item.name.clone(),
                    shop_price: item.shop_price,
                    alch_value: item.alch_value,
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(MarketService { store: Mutex::new(store), engine: Mutex::new(Engine::new(catalogue)) })
    }

    pub(crate) fn lock_store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn lock_engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn now_clock() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
    }

    /// Reference price: VWAP of the last day, else book mid, else shop.
    fn reference(&self, item: i64) -> i64 {
        let clock = Self::now_clock();
        let shop = self.lock_engine().find_item(item).map(|entry| entry.shop_price).unwrap_or(1);
        let fills: Vec<lifescape_market::PricedFill> = self
            .lock_store()
            .executions_for_item(item, clock - 86400, clock + 1)
            .unwrap_or_default()
            .iter()
            .map(|exec| lifescape_market::PricedFill {
                quantity: exec.quantity,
                price: exec.price,
                clock: exec.clock,
            })
            .collect();
        let (bid, ask) = self
            .lock_engine()
            .books
            .get(&item)
            .map(|book| book.best())
            .unwrap_or((None, None));
        reference_price(&fills, bid, ask, shop)
    }

    /// One JSON-lines request in, one reply out. Pure dispatch: every
    /// op below is also covered by the service tests over fake clients.
    pub fn handle(&self, request: &Value) -> Value {
        match request.get("op").as_str().unwrap_or("") {
            "quote" => self.op_quote(request),
            "book" => self.op_book(request),
            "history" => self.op_history(request),
            "trades" => self.op_trades(request),
            "list" => self.op_list(request),
            "place" => self.op_place(request),
            "amend" => self.op_amend(request),
            "cancel" => self.op_cancel(request),
            "my_orders" => self.op_my_orders(request),
            "my_deals" => self.op_my_deals(request),
            "deal_arrived" => self.op_deal_arrived(request),
            "deal_report" => self.op_deal_report(request),
            "fund" => self.op_fund(request),
            "tick" => self.op_tick(request),
            "stats" => self.op_stats(),
            other => object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("unknown market op '{other}'"))),
            ]),
        }
    }

    fn err(message: String) -> Value {
        object(vec![("ok", Value::Bool(false)), ("error", str_value(&message))])
    }

    fn op_quote(&self, request: &Value) -> Value {
        let item = request.get("item").as_i64().unwrap_or(-1);
        let engine = self.lock_engine();
        let Some(row) = engine.find_item(item) else {
            return Self::err(format!("unknown item {item}"));
        };
        let (bid, ask) = engine.books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        let name = row.name.clone();
        let shop = row.shop_price;
        drop(engine);
        let reference = self.reference(item);
        let last = self.lock_engine().recent.iter().rev().find(|print| print.item == item).map(|print| print.price);
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("name", str_value(&name)),
            ("bid", bid.map(Value::Int).unwrap_or(Value::Null)),
            ("ask", ask.map(Value::Int).unwrap_or(Value::Null)),
            ("reference", Value::Int(reference)),
            ("shop", Value::Int(shop)),
            ("last", last.map(Value::Int).unwrap_or(Value::Null)),
        ])
    }

    fn op_book(&self, request: &Value) -> Value {
        let item = request.get("item").as_i64().unwrap_or(-1);
        let depth = request.get("depth").as_i64().unwrap_or(10).clamp(1, 50) as usize;
        let engine = self.lock_engine();
        if engine.find_item(item).is_none() {
            return Self::err(format!("unknown item {item}"));
        }
        let (buys, sells) = match engine.books.get(&item) {
            Some(book) => (book.depth(Side::Buy, depth), book.depth(Side::Sell, depth)),
            None => (Vec::new(), Vec::new()),
        };
        let rows = |levels: Vec<(i64, i64)>| {
            Value::Arr(levels.into_iter().map(|(price, qty)| Value::Arr(vec![Value::Int(price), Value::Int(qty)])).collect())
        };
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("buys", rows(buys)),
            ("sells", rows(sells)),
        ])
    }

    fn op_history(&self, request: &Value) -> Value {
        let item = request.get("item").as_i64().unwrap_or(-1);
        if self.lock_engine().find_item(item).is_none() {
            return Self::err(format!("unknown item {item}"));
        }
        let resolution = match request.get("res").as_str().unwrap_or("1h") {
            "1d" => YieldCandleResolution::Day1,
            _ => YieldCandleResolution::Hour1,
        };
        // Half-open store windows (`clock < to`) would drop prints from
        // the current second, so the live edge sits one second out.
        let to = request.get("to").as_i64().unwrap_or_else(|| Self::now_clock() + 1);
        let default_from = match resolution {
            YieldCandleResolution::Hour1 => to - 86400,
            YieldCandleResolution::Day1 => to - 183 * 86400,
        };
        let from = request.get("from").as_i64().unwrap_or(default_from);
        let store = self.lock_store();
        let candles = store.candles_for(item, resolution, from, to).unwrap_or_default();
        let executions = store.executions_for_item(item, from, to).unwrap_or_default();
        let candle_values: Vec<Value> = candles
            .iter()
            .map(|candle| {
                object(vec![
                    ("t", Value::Int(candle.window_start)),
                    ("o", Value::Int(candle.open_price)),
                    ("h", Value::Int(candle.high_price)),
                    ("l", Value::Int(candle.low_price)),
                    ("c", Value::Int(candle.close_price)),
                    ("v", Value::Int(candle.volume)),
                ])
            })
            .collect();
        let tail: Vec<Value> = executions
            .iter()
            .rev()
            .take(30)
            .map(|exec| {
                object(vec![
                    ("q", Value::Int(exec.quantity)),
                    ("p", Value::Int(exec.price)),
                    ("t", Value::Int(exec.clock)),
                ])
            })
            .collect();
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("res", str_value(resolution.as_str())),
            ("candles", Value::Arr(candle_values)),
            ("trades", Value::Arr(tail)),
        ])
    }

    fn op_trades(&self, request: &Value) -> Value {
        let item = request.get("item").as_i64().unwrap_or(-1);
        let limit = request.get("limit").as_i64().unwrap_or(20).clamp(1, 100) as usize;
        let engine = self.lock_engine();
        let mut prints: Vec<&Print> = engine.recent.iter().filter(|print| item < 0 || print.item == item).collect();
        let total = prints.len();
        prints = prints.into_iter().skip(total.saturating_sub(limit)).collect();
        let values: Vec<Value> = prints
            .iter()
            .map(|print| {
                object(vec![
                    ("item", Value::Int(print.item)),
                    ("q", Value::Int(print.quantity)),
                    ("p", Value::Int(print.price)),
                    ("t", Value::Int(print.clock)),
                ])
            })
            .collect();
        object(vec![("ok", Value::Bool(true)), ("trades", Value::Arr(values)), ("seq", Value::Int(engine.next_seq as i64))])
    }

    fn op_list(&self, request: &Value) -> Value {
        let query = request.get("q").as_str().unwrap_or("").to_lowercase();
        let limit = request.get("limit").as_i64().unwrap_or(200).clamp(1, 2000) as usize;
        let clock = Self::now_clock();
        let engine = self.lock_engine();
        let mut rows = Vec::new();
        for row in &engine.catalogue {
            if !query.is_empty() && !row.name.to_lowercase().contains(&query) {
                continue;
            }
            rows.push((row.id, row.name.clone(), row.shop_price));
            if rows.len() >= limit {
                break;
            }
        }
        drop(engine);
        // One row per item: reference, 24h change and volume. Empty books
        // fall back to the shop price so the list never shows holes.
        let store = self.lock_store();
        let mut items = Vec::new();
        for (id, name, shop) in rows {
            let day = store.executions_for_item(id, clock - 86400, clock + 1).unwrap_or_default();
            let volume: i64 = day.iter().map(|exec| exec.quantity).sum();
            let reference = reference_price(
                &day
                    .iter()
                    .map(|exec| lifescape_market::PricedFill {
                        quantity: exec.quantity,
                        price: exec.price,
                        clock: exec.clock,
                    })
                    .collect::<Vec<_>>(),
                None,
                None,
                shop,
            );
            let change = match (day.first(), day.last()) {
                (Some(first), Some(last)) if first.price >= 1 => {
                    (last.price - first.price) as f64 / first.price as f64
                }
                _ => 0.0,
            };
            items.push(object(vec![
                ("item", Value::Int(id)),
                ("name", str_value(&name)),
                ("price", Value::Int(reference)),
                ("change24h", Value::Int((change * 1000.0).round() as i64)),
                ("volume24h", Value::Int(volume)),
            ]));
        }
        object(vec![("ok", Value::Bool(true)), ("items", Value::Arr(items))])
    }

    fn side_of(request: &Value) -> Result<Side, String> {
        match request.get("side").as_str().unwrap_or("") {
            "buy" => Ok(Side::Buy),
            "sell" => Ok(Side::Sell),
            other => Err(format!("bad side '{other}'")),
        }
    }

    fn order_side(side: Side) -> OrderSide {
        match side {
            Side::Buy => OrderSide::Buy,
            Side::Sell => OrderSide::Sell,
        }
    }

    fn op_place(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        if account.is_empty() {
            return Self::err("account is required".to_string());
        }
        let side = match Self::side_of(request) {
            Ok(side) => side,
            Err(error) => return Self::err(error),
        };
        let item = request.get("item").as_i64().unwrap_or(-1);
        let qty = request.get("qty").as_i64().unwrap_or(0);
        let price = request.get("price").as_i64().unwrap_or(0);
        if self.lock_engine().find_item(item).is_none() {
            return Self::err(format!("unknown item {item}"));
        }
        let reference = self.reference(item);
        if let Err(error) = check_price(side, price, Some(reference)) {
            return Self::err(error.to_string());
        }
        {
            let engine = self.lock_engine();
            let have_item = engine.ledger.stock_of(&account, item);
            let have_coins = engine.ledger.coins_of(&account);
            if let Err(error) = check_coverage(side, qty, price, have_item, have_coins) {
                return Self::err(error.to_string());
            }
        }
        let clock = Self::now_clock();
        let store_id = {
            let store = self.lock_store();
            let _ = store.upsert_account(&lifescape_store::AccountEcon {
                account: account.clone(),
                iron: false,
                deals_settled: 0,
                deals_failed: 0,
                market_affinity: 0.5,
                price_patience: 0.5,
                archetype: String::new(),
            });
            match store.place_order(&NewOrder {
                account: account.clone(),
                side: Self::order_side(side),
                item_id: item,
                quantity: qty,
                price,
                dormant: false,
                clock,
            }) {
                Ok(id) => id,
                Err(error) => return Self::err(error.to_string()),
            }
        };
        let order_key = store_id as u64;
        let (fills, rests) = {
            let mut engine = self.lock_engine();
            let book = engine.books.entry(item).or_insert_with(Book::new);
            match book.place(order_key, &account, item, side, qty, price) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.lock_store().cancel_order(store_id, clock).ok();
                    return Self::err(error.to_string());
                }
            }
        };
        self.lock_engine().order_item.insert(order_key, item);
        self.lock_engine().placed += 1;
        // Match-time settlement: executions persist, one deal per fill
        // carries the physical handover; the ledger moves now.
        let mut deal_ids = Vec::new();
        for fill in &fills {
            let channel = self.record_fill(fill, clock);
            match channel {
                Ok(deal_id) => deal_ids.push(Value::Int(deal_id)),
                Err(error) => return Self::err(error),
            }
        }
        self.refresh_order_rows(order_key, &fills, clock);
        let status = if rests { "open" } else { "filled" };
        object(vec![
            ("ok", Value::Bool(true)),
            ("order", Value::Int(store_id)),
            ("status", str_value(status)),
            ("fills",
                Value::Arr(
                    fills
                        .iter()
                        .map(|fill| {
                            object(vec![
                                ("q", Value::Int(fill.quantity)),
                                ("p", Value::Int(fill.price)),
                                ("buyer", str_value(&fill.buyer)),
                                ("seller", str_value(&fill.seller)),
                            ])
                        })
                        .collect(),
                )),
            ("deals", Value::Arr(deal_ids)),
        ])
    }

    /// Persists one fill: execution row, ledger move, live-tail print and
    /// an auto-convened deal (meeting tile + deadline assigned now).
    fn record_fill(&self, fill: &lifescape_market::Fill, clock: i64) -> Result<i64, String> {
        let store = self.lock_store();
        store
            .record_execution(&lifescape_store::NewExecution {
                item_id: fill.item,
                quantity: fill.quantity,
                price: fill.price,
                clock,
                server_tick: clock,
            })
            .map_err(|error| error.to_string())?;
        drop(store);
        {
            let mut engine = self.lock_engine();
            let cost = fill.quantity * fill.price;
            engine.ledger.credit_coins(&fill.buyer, -cost);
            engine.ledger.credit_stock(&fill.buyer, fill.item, fill.quantity);
            engine.ledger.credit_stock(&fill.seller, fill.item, -fill.quantity);
            engine.ledger.credit_coins(&fill.seller, cost);
            engine.matched += 1;
            engine.remember(Print { item: fill.item, quantity: fill.quantity, price: fill.price, clock });
        }
        let (tile_x, tile_y) = {
            let engine = self.lock_engine();
            let slot = (engine.deals.len() % RING.len()) as usize;
            RING[slot]
        };
        let deal_id = {
            let store = self.lock_store();
            store
                .create_deal(&NewDeal {
                    buyer: fill.buyer.clone(),
                    seller: fill.seller.clone(),
                    item_id: fill.item,
                    quantity: fill.quantity,
                    price: fill.price,
                    meeting_x: tile_x,
                    meeting_y: tile_y,
                    meeting_level: 0,
                    deadline_tick: clock + 3600,
                    clock,
                })
                .map_err(|error| error.to_string())?
        };
        self.lock_store()
            .set_deal_status(deal_id, lifescape_store::DealStatus::Convened, "", clock)
            .map_err(|error| error.to_string())?;
        let mut engine = self.lock_engine();
        if let Ok(deal) = Deal::fresh(
            deal_id as u64,
            &fill.buyer,
            &fill.seller,
            fill.item,
            fill.quantity,
            fill.price,
            tile_x,
            tile_y,
            clock + 3600,
        ) {
            let mut convened = deal;
            let _ = convened.transition(DealStatus::Convened, None);
            engine.deals.insert(deal_id as u64, convened);
            engine.arrived.insert(deal_id as u64, (false, false));
        }
        Ok(deal_id)
    }

    /// Writes post-match remaining quantities back to the store rows.
    fn refresh_order_rows(&self, incoming: u64, fills: &[lifescape_market::Fill], clock: i64) {
        let engine = self.lock_engine();
        let mut touched = vec![incoming];
        for fill in fills {
            touched.push(fill.buy_id);
            touched.push(fill.sell_id);
        }
        touched.sort_unstable();
        touched.dedup();
        let remaining: Vec<(u64, Option<i64>)> =
            touched.iter().map(|id| (*id, engine.books.values().filter_map(|book| book.remaining(*id)).next())).collect();
        drop(engine);
        let store = self.lock_store();
        for (id, left) in remaining {
            let (status, qty) = match left {
                // Untouched incoming order stays open; a resting order
                // drained by fills settles, a partial one shows matched.
                Some(left) if id == incoming && fills.is_empty() => (OrderStatus::Open, left),
                Some(left) if left > 0 => (OrderStatus::Matched, left),
                _ => (OrderStatus::Settled, 0),
            };
            let _ = store.set_order_status(id as i64, status, qty, clock);
        }
    }

    fn op_amend(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let order_id = request.get("order").as_i64().unwrap_or(-1);
        let price = request.get("price").as_i64().unwrap_or(0);
        if order_id < 1 {
            return Self::err(format!("bad order {order_id}"));
        }
        let item = match self.lock_engine().order_item.get(&(order_id as u64)).cloned() {
            Some(item) => item,
            None => return Self::err(format!("unknown order {order_id}")),
        };
        let stored = self.lock_store().get_order(order_id).unwrap_or(None);
        let Some(row) = stored else {
            return Self::err(format!("unknown order {order_id}"));
        };
        if row.account != account {
            return Self::err("order belongs to another account".to_string());
        }
        if row.status != lifescape_store::OrderStatus::Open && row.status != lifescape_store::OrderStatus::Matched {
            return Self::err(format!("order {order_id} is closed"));
        }
        let side = if row.side == OrderSide::Buy { Side::Buy } else { Side::Sell };
        let reference = self.reference(item);
        if let Err(error) = check_price(side, price, Some(reference)) {
            return Self::err(error.to_string());
        }
        let clock = Self::now_clock();
        {
            let mut engine = self.lock_engine();
            let Some(book) = engine.books.get_mut(&item) else {
                return Self::err(format!("unknown order {order_id}"));
            };
            if book.remaining(order_id as u64).is_none() {
                return Self::err(format!("order {order_id} already filled"));
            }
        }
        let store = self.lock_store();
        if store.cancel_order(order_id, clock).is_err() {
            return Self::err(format!("order {order_id} is closed"));
        }
        let remaining = self.lock_engine().books.get(&item).and_then(|book| book.remaining(order_id as u64)).unwrap_or(row.remaining);
        let replacement = match store.place_order(&NewOrder {
            account: account.clone(),
            side: row.side,
            item_id: item,
            quantity: remaining,
            price,
            dormant: false,
            clock,
        }) {
            Ok(id) => id,
            Err(error) => return Self::err(error.to_string()),
        };
        drop(store);
        let amend_outcome = {
            let mut engine = self.lock_engine();
            let Some(book) = engine.books.get_mut(&item) else {
                return Self::err(format!("unknown order {order_id}"));
            };
            book.amend(order_id as u64, price, replacement as u64)
        };
        let (fills, _rests) = match amend_outcome {
            Ok((_, fills)) => (fills, true),
            Err(error) => return Self::err(error.to_string()),
        };
        self.lock_engine().order_item.insert(replacement as u64, item);
        let mut deal_ids = Vec::new();
        for fill in &fills {
            match self.record_fill(fill, clock) {
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

    fn op_cancel(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let order_id = request.get("order").as_i64().unwrap_or(-1);
        let stored = self.lock_store().get_order(order_id).unwrap_or(None);
        let Some(row) = stored else {
            return Self::err(format!("unknown order {order_id}"));
        };
        if row.account != account {
            return Self::err("order belongs to another account".to_string());
        }
        let clock = Self::now_clock();
        if self.lock_store().cancel_order(order_id, clock).is_err() {
            return Self::err(format!("order {order_id} is closed"));
        }
        // Named binding first: an `if let` scrutinee temporary would keep
        // the engine guard alive across the body and deadlock the book.
        let item = self.lock_engine().order_item.get(&(order_id as u64)).cloned();
        if let Some(item) = item {
            if let Some(book) = self.lock_engine().books.get_mut(&item) {
                let _ = book.cancel(order_id as u64);
            }
        }
        object(vec![("ok", Value::Bool(true)), ("cancelled", Value::Int(order_id))])
    }

    fn op_my_orders(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let store = self.lock_store();
        let rows = store.account_orders(account).unwrap_or_default();
        let values: Vec<Value> = rows
            .iter()
            .map(|row| {
                object(vec![
                    ("order", Value::Int(row.order_id)),
                    ("side",
                        str_value(match row.side {
                            OrderSide::Buy => "buy",
                            OrderSide::Sell => "sell",
                        })),
                    ("item", Value::Int(row.item_id)),
                    ("qty", Value::Int(row.quantity)),
                    ("left", Value::Int(row.remaining)),
                    ("price", Value::Int(row.price)),
                    ("status", str_value(match row.status {
                        OrderStatus::Open => "open",
                        OrderStatus::Matched => "matched",
                        OrderStatus::Settled => "settled",
                        OrderStatus::Cancelled => "cancelled",
                    })),
                ])
            })
            .collect();
        object(vec![("ok", Value::Bool(true)), ("orders", Value::Arr(values))])
    }

    fn op_my_deals(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let store = self.lock_store();
        let rows = store.open_deals_for_account(account).unwrap_or_default();
        let values: Vec<Value> = rows
            .iter()
            .map(|row| {
                object(vec![
                    ("deal", Value::Int(row.deal_id)),
                    ("buyer", str_value(&row.buyer)),
                    ("seller", str_value(&row.seller)),
                    ("item", Value::Int(row.item_id)),
                    ("qty", Value::Int(row.quantity)),
                    ("price", Value::Int(row.price)),
                    ("x", Value::Int(row.meeting_x)),
                    ("y", Value::Int(row.meeting_y)),
                    ("status", str_value(match row.status {
                        lifescape_store::DealStatus::Matched => "matched",
                        lifescape_store::DealStatus::Convened => "convened",
                        lifescape_store::DealStatus::BothArrived => "both_arrived",
                        lifescape_store::DealStatus::Trading => "trading",
                        lifescape_store::DealStatus::Settled => "settled",
                        lifescape_store::DealStatus::Failed => "failed",
                    })),
                    ("reason", str_value(&row.fail_reason)),
                    ("deadline", Value::Int(row.deadline_tick)),
                ])
            })
            .collect();
        object(vec![("ok", Value::Bool(true)), ("deals", Value::Arr(values))])
    }

    fn op_deal_arrived(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let deal_id = request.get("deal").as_i64().unwrap_or(-1);
        let (buyer, seller, status) = {
            let engine = self.lock_engine();
            let Some(deal) = engine.deals.get(&(deal_id as u64)) else {
                return Self::err(format!("unknown deal {deal_id}"));
            };
            if account != deal.buyer && account != deal.seller {
                return Self::err("not your deal".to_string());
            }
            if deal.status != DealStatus::Convened {
                return Self::err(format!("deal {deal_id} is {}", deal.status.as_str()));
            }
            (deal.buyer.clone(), deal.seller.clone(), deal.status.as_str().to_string())
        };
        let mut engine = self.lock_engine();
        let mut marks = engine.arrived.get(&(deal_id as u64)).cloned().unwrap_or((false, false));
        if account == buyer {
            marks.0 = true;
        } else if account == seller {
            marks.1 = true;
        }
        engine.arrived.insert(deal_id as u64, marks);
        let both = marks.0 && marks.1;
        drop(engine);
        if both {
            let mut engine = self.lock_engine();
            if let Some(deal) = engine.deals.get_mut(&(deal_id as u64)) {
                if deal.transition(DealStatus::BothArrived, None).is_ok() {
                    self.lock_store()
                        .set_deal_status(deal_id, lifescape_store::DealStatus::BothArrived, "", Self::now_clock())
                        .ok();
                }
            }
            return object(vec![("ok", Value::Bool(true)), ("deal", Value::Int(deal_id)), ("status", str_value("both_arrived"))]);
        }
        object(vec![("ok", Value::Bool(true)), ("deal", Value::Int(deal_id)), ("status", str_value(&status))])
    }

    fn op_deal_report(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let deal_id = request.get("deal").as_i64().unwrap_or(-1);
        let outcome = request.get("outcome").as_str().unwrap_or("").to_string();
        let reason = request.get("reason").as_str().unwrap_or("").to_string();
        let next = match outcome.as_str() {
            "trading" => DealStatus::Trading,
            "settled" => DealStatus::Settled,
            "failed" => DealStatus::Failed,
            _ => return Self::err(format!("bad outcome '{outcome}'")),
        };
        let fail = if next == DealStatus::Failed {
            match FailReason::parse(&reason) {
                Ok(why) => Some(why),
                Err(error) => return Self::err(error.to_string()),
            }
        } else if !reason.is_empty() {
            return Self::err("reason only with failed".to_string());
        } else {
            None
        };
        let clock = Self::now_clock();
        // Snapshot the parties first: the ledger refund below needs them
        // without holding the engine lock across the store calls.
        let (buyer, seller, item, qty, price) = {
            let engine = self.lock_engine();
            let Some(deal) = engine.deals.get(&(deal_id as u64)) else {
                return Self::err(format!("unknown deal {deal_id}"));
            };
            if account != deal.buyer && account != deal.seller {
                return Self::err("not your deal".to_string());
            }
            (deal.buyer.clone(), deal.seller.clone(), deal.item, deal.quantity, deal.price)
        };
        let moved = {
            let mut engine = self.lock_engine();
            let Some(deal) = engine.deals.get_mut(&(deal_id as u64)) else {
                return Self::err(format!("unknown deal {deal_id}"));
            };
            match deal.transition(next, fail) {
                Ok(()) => true,
                Err(error) => return Self::err(error.to_string()),
            }
        };
        assert!(moved);
        let store_status = match next {
            DealStatus::Trading => lifescape_store::DealStatus::Trading,
            DealStatus::Settled => lifescape_store::DealStatus::Settled,
            _ => lifescape_store::DealStatus::Failed,
        };
        self.lock_store().set_deal_status(deal_id, store_status, fail.map(|why| why.as_str()).unwrap_or(""), clock).ok();
        if next == DealStatus::Failed {
            // Refund the match-time transfer; the agreement is void.
            let mut engine = self.lock_engine();
            let cost = qty * price;
            engine.ledger.credit_coins(&buyer, cost);
            engine.ledger.credit_stock(&buyer, item, -qty);
            engine.ledger.credit_stock(&seller, item, qty);
            engine.ledger.credit_coins(&seller, -cost);
        }
        if next == DealStatus::Settled || next == DealStatus::Failed {
            let store = self.lock_store();
            let _ = store.record_deal_outcome(&buyer, next == DealStatus::Settled);
            let _ = store.record_deal_outcome(&seller, next == DealStatus::Settled);
        }
        object(vec![("ok", Value::Bool(true)), ("deal", Value::Int(deal_id)), ("status", str_value(next.as_str()))])
    }

    fn op_fund(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        if account.is_empty() {
            return Self::err("account is required".to_string());
        }
        let mut engine = self.lock_engine();
        if let Some(coins) = request.get("coins").as_i64() {
            engine.ledger.credit_coins(&account, coins);
        }
        if let (Some(item), Some(qty)) = (request.get("item").as_i64(), request.get("qty").as_i64()) {
            if item >= 0 {
                engine.ledger.credit_stock(&account, item, qty);
            }
        }
        let coins = engine.ledger.coins_of(&account);
        object(vec![("ok", Value::Bool(true)), ("account", str_value(&account)), ("coins", Value::Int(coins))])
    }

    fn op_tick(&self, request: &Value) -> Value {
        let since = request.get("since").as_i64().unwrap_or(0);
        let engine = self.lock_engine();
        let fresh: Vec<Value> = engine
            .recent
            .iter()
            .filter(|print| print.clock > since)
            .map(|print| {
                object(vec![
                    ("item", Value::Int(print.item)),
                    ("q", Value::Int(print.quantity)),
                    ("p", Value::Int(print.price)),
                    ("t", Value::Int(print.clock)),
                ])
            })
            .collect();
        object(vec![("ok", Value::Bool(true)), ("seq", Value::Int(engine.next_seq as i64)), ("trades", Value::Arr(fresh))])
    }

    fn op_stats(&self) -> Value {
        let engine = self.lock_engine();
        object(vec![
            ("ok", Value::Bool(true)),
            ("items", Value::Int(engine.catalogue.len() as i64)),
            ("books", Value::Int(engine.books.len() as i64)),
            ("orders", Value::Int(engine.order_item.len() as i64)),
            ("placed", Value::Int(engine.placed as i64)),
            ("matched", Value::Int(engine.matched as i64)),
            ("deals", Value::Int(engine.deals.len() as i64)),
        ])
    }
}

fn registry() -> &'static OnceLock<Arc<MarketService>> {
    static SERVICE: OnceLock<Arc<MarketService>> = OnceLock::new();
    &SERVICE
}

/// Handle one market request on the daemon service. Down service is a
/// loud error, never an empty book.
pub fn handle(request: &Value) -> Value {
    match registry().get() {
        Some(service) => service.handle(request),
        None => object(vec![
            ("ok", Value::Bool(false)),
            ("error", str_value("market service not running")),
        ]),
    }
}

/// Serve one worker connection: a single JSON-lines frame in, a single
/// frame out, then close (CLI shape, no thread per message past the
/// bounded pool).
fn serve_one(service: Arc<MarketService>, stream: UnixStream) {
        let request = match crate::daemon_pool::read_frame(&stream) {
        Some(frame) if frame.as_obj().is_some() => frame,
        _ => object(vec![]),
    };
    let reply = service.handle(&request);
    let mut buf = lifescape_json::stringify(&reply);
    buf.push('\n');
    let _ = (&stream).write_all(buf.as_bytes());
}

/// Starts the daemon market: catalogue from the dumps, store opened
/// here alone, demo history seeded when empty, socket served on the
/// bounded pool. Idempotent: a live socket refuses a second server.
pub fn start() -> Result<Arc<MarketService>, String> {
    let items = catalogue();
    let service = Arc::new(MarketService::open(&db_path(), items).map_err(|error| format!("market open: {error}"))?);
    crate::market_seed::seed_demo_if_empty(&service).map_err(|error| format!("market seed: {error}"))?;
    let sock = socket_path();
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            return Err(format!("market already serving on {}", sock.display()));
        }
        let _ = std::fs::remove_file(&sock);
    }
    let listener = UnixListener::bind(&sock).map_err(|error| format!("bind {}: {error}", sock.display()))?;
    let serving = service.clone();
    std::thread::Builder::new()
        .name("market-accept".to_string())
        .spawn(move || {
            crate::daemon_pool::serve_pooled(
                listener,
                Arc::new(move |stream: UnixStream| serve_one(serving.clone(), stream)),
            );
        })
        .map_err(|error| error.to_string())?;
    let _ = registry().set(service.clone());
    Ok(service)
}
