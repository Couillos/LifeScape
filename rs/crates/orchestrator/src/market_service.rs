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

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{check_coverage, check_price, reference_price, Order};
use crate::market_seed::{catalogue, CatalogueItem};
use lifescape_market::{Book, Deal, Side};
use lifescape_store::{NewOrder, OrderSide, OrderStatus, Store, YieldCandleResolution};

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
/// Static fallback when nav was down at boot and no ring computed: the
/// convene allocator prefers the computed `Engine::ring`.
pub(crate) const RING: [(i64, i64); 8] =
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
    /// Handover ring around the fountain (computed at startup via the
    /// nav `tiles_flags` batch op; empty when nav was down at boot, the
    /// static fallback ring applies per deal then).
    pub(crate) ring: Vec<(i64, i64)>,
    pub(crate) ring_busy: HashSet<(i64, i64)>,
    /// Convened handover tiles per deal: (buyer tile, seller tile).
    pub(crate) deal_tiles: HashMap<u64, ((i64, i64), (i64, i64))>,
    /// Sweep wall per convened deal (daemon wall seconds): session
    /// ticks are per-login epochs, never comparable across bots.
    pub(crate) deal_sweep_wall: HashMap<u64, f64>,
    /// Consumed book rows per deal for a priority-preserving relist when
    /// the handover fails (original `seq` kept).
    pub(crate) relist_rows: HashMap<u64, Vec<Order>>,
    /// Accounts with a live convocation: one at a time, regrouped.
    pub(crate) convened_accounts: HashSet<String>,
    /// First convene-tick sighting per matched deal (daemon wall) for
    /// the grouping window.
    pub(crate) pending_since_wall: HashMap<u64, f64>,
    pub(crate) next_seq: u64,
    pub(crate) placed: u64,
    pub(crate) matched: u64,
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
            ring: Vec::new(),
            ring_busy: HashSet::new(),
            deal_tiles: HashMap::new(),
            deal_sweep_wall: HashMap::new(),
            relist_rows: HashMap::new(),
            convened_accounts: HashSet::new(),
            pending_since_wall: HashMap::new(),
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
            "pending" => self.op_pending(request),
            "convene" => self.op_convene(request),
            "sweep" => self.op_sweep(request),
            "fund" => self.op_fund(request),
            "tick" => self.op_tick(request),
            "stats" => self.op_stats(),
            other => object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("unknown market op '{other}'"))),
            ]),
        }
    }

    pub(crate) fn err(message: String) -> Value {
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
        let (matched, rests) = {
            let mut engine = self.lock_engine();
            let book = engine.books.entry(item).or_insert_with(Book::new);
            // Pre-match snapshot for the priority-preserving relist: the
            // resting rows keep their `seq`, the taker its arrival rank.
            let pre: HashMap<u64, Order> = book.orders().into_iter().map(|order| (order.id, order)).collect();
            let taker_seq = book.next_seq();
            match book.place(order_key, &account, item, side, qty, price) {
                Ok((fills, rests)) => {
                    let matched: Vec<(lifescape_market::Fill, Vec<Order>)> = fills
                        .into_iter()
                        .map(|fill| {
                            let rows =
                                crate::market_convene::relist_for_fill(&pre, taker_seq, side, &account, price, &fill);
                            (fill, rows)
                        })
                        .collect();
                    (matched, rests)
                }
                Err(error) => {
                    self.lock_store().cancel_order(store_id, clock).ok();
                    return Self::err(error.to_string());
                }
            }
        };
        let fills: Vec<lifescape_market::Fill> = matched.iter().map(|(fill, _)| fill.clone()).collect();
        self.lock_engine().order_item.insert(order_key, item);
        self.lock_engine().placed += 1;
        // Match-time settlement: executions persist, one deal per fill
        // carries the physical handover; the ledger moves now.
        let mut deal_ids = Vec::new();
        for (fill, rows) in &matched {
            let channel = self.record_fill(fill, rows.clone(), clock);
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
            // Same pre-match snapshot as `op_place`: the amended order is
            // the taker here, with its amended price.
            let pre: HashMap<u64, Order> = book.orders().into_iter().map(|order| (order.id, order)).collect();
            let taker_seq = book.next_seq();
            match book.amend(order_id as u64, price, replacement as u64) {
                Ok((_, fills)) => fills
                    .into_iter()
                    .map(|fill| {
                        let rows =
                            crate::market_convene::relist_for_fill(&pre, taker_seq, side, &account, price, &fill);
                        (fill, rows)
                    })
                    .collect::<Vec<(lifescape_market::Fill, Vec<Order>)>>(),
                Err(error) => return Self::err(error.to_string()),
            }
        };
        let fills: Vec<lifescape_market::Fill> = amend_outcome.iter().map(|(fill, _)| fill.clone()).collect();
        self.lock_engine().order_item.insert(replacement as u64, item);
        let mut deal_ids = Vec::new();
        for (fill, rows) in &amend_outcome {
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

/// Daemon-side handle for the convene loop (same process, no socket).
pub(crate) fn shared() -> Option<Arc<MarketService>> {
    registry().get().cloned()
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
    // Handover ring around the fountain via the nav batch op (walkable,
    // no busts or stalls). Read-only nav use; a down service leaves the
    // ring empty and every deal falls back to the static ring.
    if let Some(snapshot) = crate::plan_service::snapshot() {
        let ring = crate::market_convene::build_ring_from_snapshot(&snapshot);
        service.lock_engine().ring = ring;
    }
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
