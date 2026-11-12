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
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{check_coverage, check_price, reference_price, Order};
use crate::jobs::Job;
use crate::market_seed::CatalogueItem;
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

    /// Funding mirror of the last pushed holdings (daemon sync, see
    /// `market_convene`): SET semantics, not credit. Live bots spend
    /// outside the market (shops, deaths), so only the fresh snapshot
    /// is truthful; match-time settlement still moves the ledger between
    /// syncs. Known limit, documented in decisions: two quick buys can
    /// over-commit against the same coins until the handover updates the
    /// snapshot; the physical trade double-checks real items, so this
    /// degrades into failed deals with a reliability penalty, never
    /// into lost items.
    pub(crate) fn sync_account(&mut self, account: &str, coins: i64, stock: &[(i64, i64)]) {
        self.coins.insert(account.to_string(), coins.max(0));
        self.stock.retain(|(owner, _item), _qty| owner != account);
        for (item, qty) in stock {
            if *qty > 0 {
                self.stock.insert((account.to_string(), *item), *qty);
            }
        }
    }
}

/// Coins and per-object stock of one live snapshot (pack, bank, worn:
/// numeric ids). The bank row is the last known bank (the heartbeat keeps
/// it once closed): banked coins fund bids, banked stock covers asks.
pub(crate) fn ledger_holdings(live: &Value, coins_id: i64) -> (i64, Vec<(i64, i64)>) {
    let mut coins = 0i64;
    let mut stock: HashMap<i64, i64> = HashMap::new();
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            if !matches!(entry, Value::Obj(_)) {
                continue;
            }
            let id = entry.get("id").as_i64().unwrap_or(-1);
            let count = entry.get("count").as_i64().unwrap_or(1).max(0);
            if id < 0 {
                continue;
            }
            if id == coins_id {
                coins += count;
            } else {
                *stock.entry(id).or_insert(0) += count;
            }
        }
    }
    let mut stock: Vec<(i64, i64)> = stock.into_iter().collect();
    stock.sort();
    (coins, stock)
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
    #[cfg(test)]
    pub(crate) fn rebuild_candles(&self, item: i64, resolution: lifescape_store::YieldCandleResolution) -> Result<(), String> {
        self.lock_store().rebuild_candles(item, resolution).map(|_| ()).map_err(|error| error.to_string())
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
        let service = MarketService { store: Mutex::new(store), engine: Mutex::new(Engine::new(catalogue)) };
        service.reload_books();
        Ok(service)
    }

    /// Rebuilds the in-memory books from the persisted carnet after a
    /// restart: resting (`open`, plus partially filled `matched` rows
    /// stranded without their deals) orders rest again under their store
    /// ids, ranked by creation clock so price-time priority replays the
    /// pre-shutdown queue. Stranded non-final deals fail loudly (both
    /// parties disconnected with the daemon). Without this a restart
    /// orphans every order: visible in `my_orders`, invisible to
    /// matching, and the market silently dies (seen live, L11).
    pub(crate) fn reload_books(&self) {
        let clock = Self::now_clock();
        let stale_deals = self.lock_store().fail_stale_deals("daemon restart", clock).unwrap_or(0);
        if stale_deals > 0 {
            crate::logln!("market reload: failed {stale_deals} stale in-flight deals (daemon restart)");
        }
        let rows = match self.lock_store().resting_orders() {
            Ok(rows) => rows,
            Err(error) => {
                crate::logln!("market reload: no book rebuild, store read failed: {error}");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let mut reopened = 0usize;
        {
            let mut engine = self.lock_engine();
            for (seq, row) in rows.iter().enumerate() {
                let side = match row.side {
                    lifescape_store::OrderSide::Buy => Side::Buy,
                    lifescape_store::OrderSide::Sell => Side::Sell,
                };
                let book = engine.books.entry(row.item_id).or_insert_with(Book::new);
                book.relist(Order {
                    id: row.order_id as u64,
                    account: row.account.clone(),
                    item: row.item_id,
                    side,
                    quantity: row.quantity,
                    remaining: row.remaining.max(0),
                    price: row.price,
                    seq: seq as u64,
                });
                engine.order_item.insert(row.order_id as u64, row.item_id);
            }
        }
        // Stranded `matched` rows rejoin as plain resting orders: their
        // deals died with the daemon and no handover can resume them.
        let store = self.lock_store();
        for row in &rows {
            if row.status == lifescape_store::OrderStatus::Matched
                && store
                    .set_order_status(row.order_id, lifescape_store::OrderStatus::Open, row.remaining.max(0), clock)
                    .is_ok()
            {
                reopened += 1;
            }
        }
        crate::logln!(
            "market reload: {} resting orders back in {} books ({reopened} stranded matched reopened)",
            rows.len(),
            self.lock_engine().books.len()
        );
    }

    /// Funds the ledger from the last pushed holdings (L11): the
    /// coverage check in `op_place` runs against the ledger, but nothing
    /// ever credited live bots, so every live order failed coverage. Each
    /// convene round mirrors inv+bank+worn item counts (numeric ids from
    /// the snapshot) into the ledger; match-time settlement still moves
    /// it between rounds. Stale (>30 s) or missing snapshots skip: no
    /// funding without a live witness. SET semantics over-commit window
    /// documented on `sync_account`.
    pub(crate) fn sync_ledger_from_jobs(&self, jobs: &[std::sync::Arc<Job>]) {
        let coins_id = lifescape_gamedata::objs_by_name("Coins")
            .into_iter()
            .next()
            .map(|entry| entry.obj_id as i64)
            .unwrap_or(995);
        let now = crate::jobs::now_secs();
        for job in jobs {
            let live = match job.inner.try_lock().map(|inner| inner.live.clone()) {
                Ok(Some(live)) => live,
                _ => continue,
            };
            if !live.get("updated").as_f64().is_some_and(|at| now - at < 30.0) {
                continue;
            }
            let (coins, stock) = ledger_holdings(&live, coins_id);
            self.lock_engine().ledger.sync_account(&job.account, coins, &stock);
        }
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
    /// Reference of one item (PRD3 lot 2): 24 h VWAP, else the last
    /// print ever, else the book midpoint, else 1 coin. The shop price
    /// never anchors it.
    pub(crate) fn reference(&self, item: i64) -> lifescape_market::Reference {
        let clock = Self::now_clock();
        let all = self.lock_store().executions_for_item(item, 0, clock + 1).unwrap_or_default();
        let last_print = all.iter().max_by_key(|exec| exec.clock).map(|exec| exec.price);
        let fills: Vec<lifescape_market::PricedFill> = all
            .iter()
            .filter(|exec| exec.clock >= clock - 86400)
            .map(|exec| lifescape_market::PricedFill { quantity: exec.quantity, price: exec.price, clock: exec.clock })
            .collect();
        let (bid, ask) = self.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        reference_price(&fills, last_print, bid, ask)
    }

    /// The guard anchor: only a traded reference caps an order.
    fn guard_anchor(&self, item: i64) -> Option<i64> {
        let reference = self.reference(item);
        reference.traded.then_some(reference.price)
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
            "recent" => self.op_recent(request),
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
        // Last print from the store (durable): the in-memory ring forgot
        // it at every daemon restart.
        let clock = Self::now_clock();
        let last = self
            .lock_store()
            .executions_for_item(item, 0, clock + 1)
            .unwrap_or_default()
            .iter()
            .max_by_key(|exec| exec.clock)
            .map(|exec| exec.price);
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("name", str_value(&name)),
            ("bid", bid.map(Value::Int).unwrap_or(Value::Null)),
            ("ask", ask.map(Value::Int).unwrap_or(Value::Null)),
            ("reference", Value::Int(reference.price)),
            ("traded", Value::Bool(reference.traded)),
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
        let mut store = self.lock_store();
        let candles = store.candles_or_rebuild(item, resolution, from, to).unwrap_or_default();
        let executions = store.executions_for_item(item, from, to).unwrap_or_default();
        // Execution count per candle window (tooltip §6.2, `n`): the
        // candles table carries OHLCV only, so count the window's fills.
        let width = resolution.width_seconds();
        let candle_values: Vec<Value> = candles
            .iter()
            .map(|candle| {
                let count = executions
                    .iter()
                    .filter(|exec| exec.clock >= candle.window_start && exec.clock < candle.window_start + width)
                    .count() as i64;
                object(vec![
                    ("t", Value::Int(candle.window_start)),
                    ("o", Value::Int(candle.open_price)),
                    ("h", Value::Int(candle.high_price)),
                    ("l", Value::Int(candle.low_price)),
                    ("c", Value::Int(candle.close_price)),
                    ("v", Value::Int(candle.volume)),
                    ("n", Value::Int(count)),
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
        let traded_only = request.get("traded_only").as_i64().unwrap_or(0) == 1;
        // `with_orders=1`: keep goods with >= 1 open order (either side).
        let with_orders = request.get("with_orders").as_i64().unwrap_or(0) == 1;
        let clock = Self::now_clock();
        // One row per displayed name: base objects and their banknotes
        // (`cert_*` rows share the display) trade as a single named good —
        // physical delivery already settles both forms as notes. Members
        // keep catalogue order, so a volume tie falls back to the base.
        let engine = self.lock_engine();
        let mut groups: std::collections::BTreeMap<String, Vec<(i64, i64)>> =
            std::collections::BTreeMap::new();
        for row in &engine.catalogue {
            if !query.is_empty() && !row.name.to_lowercase().contains(&query) {
                continue;
            }
            groups.entry(row.name.clone()).or_default().push((row.id, row.shop_price));
        }
        // Open orders and open quantity (remaining units, both sides) per form.
        let open_orders: std::collections::HashMap<i64, (usize, i64)> = engine
            .books
            .iter()
            .map(|(item, book)| {
                let orders = book.orders();
                (*item, (orders.len(), orders.iter().map(|order| order.remaining).sum::<i64>()))
            })
            .filter(|(_, (count, _))| *count > 0)
            .collect();
        drop(engine);
        // `traded_only=1`: keep goods with >= 1 execution ever (not 24h volume).
        let traded: std::collections::HashSet<i64> = if traded_only {
            self.lock_store().items_with_executions().unwrap_or_default().into_iter().collect()
        } else { std::collections::HashSet::new() };
        // One row per named good: 24h volume summed over every form,
        // reference and change from the merged fills, canonical id = the
        // form carrying the volume (base on ties). Sorted by descending
        // volume so the traded goods surface first.
        let store = self.lock_store();
        let mut items = Vec::new();
        for (name, members) in &groups {
            if traded_only && !members.iter().any(|member| traded.contains(&member.0)) {
                continue;
            }
            let order_count: usize = members.iter().filter_map(|member| open_orders.get(&member.0)).map(|(count, _)| count).sum();
            let open_qty: i64 = members.iter().filter_map(|member| open_orders.get(&member.0)).map(|(_, qty)| qty).sum();
            if with_orders && order_count == 0 {
                continue;
            }
            let mut volume = 0i64;
            let mut fills: Vec<lifescape_market::PricedFill> = Vec::new();
            let mut volumes: Vec<(i64, i64)> = Vec::new();
            for (id, _shop) in members {
                let day = store.executions_for_item(*id, clock - 86400, clock + 1).unwrap_or_default();
                let member_volume: i64 = day.iter().map(|exec| exec.quantity).sum();
                volume += member_volume;
                volumes.push((*id, member_volume));
                fills.extend(day.iter().map(|exec| lifescape_market::PricedFill {
                    quantity: exec.quantity,
                    price: exec.price,
                    clock: exec.clock,
                }));
            }
            fills.sort_by_key(|fill| fill.clock);
            volumes.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
            let canonical = volumes.first().map(|member| member.0).unwrap_or(-1);
            let shop = members
                .iter()
                .find(|member| member.0 == canonical)
                .map(|member| member.1)
                .unwrap_or(1);
            let _ = shop;
            let reference = reference_price(&fills, fills.last().map(|fill| fill.price), None, None).price;
            let change = match (fills.first(), fills.last()) {
                (Some(first), Some(last)) if first.price >= 1 => {
                    (last.price - first.price) as f64 / first.price as f64
                }
                _ => 0.0,
            };
            items.push((name.clone(), canonical, reference, change, volume, order_count, open_qty));
        }
        // Descending 24h volume, then the largest open quantity, then the name.
        items.sort_by(|left, right| {
            right.4.cmp(&left.4).then(right.6.cmp(&left.6)).then(left.0.cmp(&right.0)).then(left.1.cmp(&right.1))
        });
        let rows: Vec<Value> = items
            .into_iter()
            .take(limit)
            .map(|(name, id, reference, change, volume, order_count, open_qty)| {
                object(vec![
                    ("orders", Value::Int(order_count as i64)),
                    ("open_qty", Value::Int(open_qty)),
                    ("item", Value::Int(id)),
                    ("name", str_value(&name)),
                    ("price", Value::Int(reference)),
                    ("change24h", Value::Int((change * 1000.0).round() as i64)),
                    ("volume24h", Value::Int(volume)),
                ])
            })
            .collect();
        object(vec![("ok", Value::Bool(true)), ("items", Value::Arr(rows))])
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
        // AGENTS rule 13: admin's cheated goods and coins never reach
        // another account, so admin never trades on the market.
        if account.eq_ignore_ascii_case("admin") {
            return Self::err("admin never trades: cheated goods stay out of the economy (rule 13)".to_string());
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
        if let Err(error) = check_price(side, price, self.guard_anchor(item)) {
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
            // First order creates the economic profile; later orders
            // never reset its counters (PRD3 lot 0: every place used to
            // zero deals_settled / deals_failed and the reliability).
            if store.get_account(&account).ok().flatten().is_none() {
                let _ = store.upsert_account(&lifescape_store::AccountEcon {
                    account: account.clone(),
                    iron: false,
                    deals_settled: 0,
                    deals_failed: 0,
                    market_affinity: 0.5,
                    price_patience: 0.5,
                    archetype: String::new(),
                });
            }
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
        if rests {
            if let Some(bounds) = crate::market_reprice::bounds_from_request(request, &account, side == Side::Sell, price) {
                crate::market_reprice::register(store_id, bounds);
            }
        }
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
        if let Err(error) = check_price(side, price, self.guard_anchor(item)) {
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

    /// Cancels every open order of `account` on `item` and `side`
    /// (daemon-side, no socket): the brain calls it when a fill wait
    /// expires, so a retry never stacks a second resting order beside
    /// the stale one. Returns how many orders it closed.
    pub(crate) fn cancel_open_for(&self, account: &str, item: i64, side: OrderSide) -> usize {
        let rows = self.lock_store().account_orders(account).unwrap_or_default();
        let mut closed = 0;
        for row in rows.iter().filter(|row| row.item_id == item && row.side == side && row.status == OrderStatus::Open) {
            let reply = self.op_cancel(&object(vec![("account", str_value(account)), ("order", Value::Int(row.order_id))]));
            if reply.get("ok").as_bool() == Some(true) {
                closed += 1;
            }
        }
        closed
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

pub use crate::market_daemon::{handle, shared, start};
