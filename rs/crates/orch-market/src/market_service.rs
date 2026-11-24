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

use lifescape_scripts::market_ops::MarketOp;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{check_coverage, check_price, LastFill, Order, PRICE_FILLS};
use crate::market_seed::CatalogueItem;
use lifescape_market::{Book, Deal, Side};
use lifescape_orch_core::store_writer::{StoreWriter, WriteGuard};
use lifescape_store::{NewOrder, OrderSide, OrderStatus, Store, YieldCandleResolution};

/// Socket the workers dial (beside `session-<account>.sock`).
pub fn socket_path() -> PathBuf {
    crate::paths::run_dir().join("market.sock")
}

/// Database file, opened by the daemon alone.
pub fn db_path() -> PathBuf {
    crate::paths::store_db()
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
pub struct Ledger {
    pub(crate) coins: HashMap<String, i64>,
    pub(crate) stock: HashMap<(String, i64), i64>,
}

impl Ledger {
    pub(crate) fn coins_of(&self, account: &str) -> i64 {
        self.coins.get(account).cloned().unwrap_or(0)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn stock_of(&self, account: &str, item: i64) -> i64 {
        self.stock.get(&(account.to_string(), item)).cloned().unwrap_or(0)
    }

    /// Every `(object, units)` the ledger holds for `account`.
    pub(crate) fn stock_rows(&self, account: &str) -> Vec<(i64, i64)> {
        self.stock.iter().filter(|((owner, _), _)| owner == account).map(|((_, item), qty)| (*item, *qty)).collect()
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
    pub fn sync_account(&mut self, account: &str, coins: i64, stock: &[(i64, i64)]) {
        self.coins.insert(account.to_string(), coins.max(0));
        self.stock.retain(|(owner, _item), _qty| owner != account);
        for (item, qty) in stock {
            if *qty > 0 {
                self.stock.insert((account.to_string(), *item), *qty);
            }
        }
    }
}

/// Recent print kept for the WS live tail (no cross-item store query).
#[derive(Clone, Debug)]
pub(crate) struct Print {
    pub(crate) item: i64,
    pub(crate) quantity: i64,
    pub(crate) price: i64,
    pub(crate) clock: i64,
    /// Taker side of the settled deal (`None`: unknown, pre-split).
    pub(crate) taker: Option<OrderSide>,
    /// The deal's buyer and seller (user 2026-11-18, « Last trades »).
    pub(crate) buyer: String,
    pub(crate) seller: String,
}

pub struct Engine {
    pub(crate) catalogue: Vec<CatalogueItem>,
    pub books: HashMap<i64, Book>,
    pub(crate) order_item: HashMap<u64, i64>,
    pub(crate) deals: HashMap<u64, Deal>,
    pub(crate) arrived: HashMap<u64, (bool, bool)>,
    pub ledger: Ledger,
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
    /// The seller's walk limit per deal (its order's `ask_limit`, schema
    /// 11), carried by the relist of a failed handover.
    pub(crate) relist_ask_limits: HashMap<u64, i64>,
    /// Accounts with a live convocation: one at a time, regrouped.
    pub(crate) convened_accounts: HashSet<String>,
    /// First convene-tick sighting per matched deal (daemon wall) for
    /// the grouping window.
    pub(crate) pending_since_wall: HashMap<u64, f64>,
    pub(crate) next_seq: u64,
    pub(crate) placed: u64,
    pub(crate) matched: u64,
    /// Repricing bounds per resting order of THIS market (it was a
    /// process-wide table keyed by order id: two markets — parallel tests
    /// on fresh databases — overwrote each other's bounds).
    pub quote_bounds: HashMap<i64, crate::market_reprice::QuoteBounds>,
    /// Failed-deal pairs kept apart until a daemon wall instant
    /// (`market_uncross`, sorted account pair).
    pub(crate) pair_cooldowns: HashMap<(String, String), f64>,
    /// Each seller's own last fill per item with how long its listing
    /// rested (`market_seller::remember_fill`): the next lot opens over a
    /// fast fill. The bot's own data only; the store's deals give the
    /// price back after a restart (speed then unknown).
    pub(crate) own_sells: HashMap<(String, i64), lifescape_market::OwnFill>,
    /// Order ids adopted by demand pricing once (`market_reprice::adopt`
    /// is idempotent: never re-adopted, never re-logged).
    pub adopted: HashSet<i64>,
    /// Last `PRICE_FILLS` fills (price, quantity) per item, oldest first
    /// (the current price and its impact size, user 2026-11-14): loaded
    /// once per item from one indexed read, then pushed on every settled
    /// fill (`settle_print`).
    pub(crate) last_fills: HashMap<i64, VecDeque<LastFill>>,
    /// Store clock of each item's newest fill (plan prix §3: a stale
    /// print yields to a two-sided book), loaded on first use.
    pub(crate) newest_fill: HashMap<i64, i64>,
    /// Settled fills counted since start: a ring loaded from the store
    /// while a fill settled is not cached (it could miss that fill).
    pub(crate) fills_settled: u64,
    /// Units each account's open quests keep, per base object
    /// (`brain_quest_gate::publish_reserved`, every brain pass): never
    /// offered for sale (`free_coverage`), so a quest object is never
    /// locked in the bank by a listing (bh99tw2f, 2026-11-14).
    pub(crate) quest_reserved: HashMap<String, std::collections::BTreeMap<i64, i64>>,
    /// Price patience per account: the life's drawn trait (plan M3),
    /// read once (`market_reprice::account_patience`).
    pub(crate) patience: HashMap<String, f64>,
    /// Deals each account let expire in a row (`timeout` it reported
    /// itself); a settled deal resets it (decision D8).
    pub(crate) timeout_streak: HashMap<String, u32>,
    /// Gifts to the market makers, deals of their own kind (`funding_gifts`).
    pub(crate) gifts: crate::funding_gifts::GiftBook,
    /// Grouped-auction modes and pending walks (`market_auction`).
    pub(crate) auction: crate::market_auction::AuctionState,
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
            relist_ask_limits: HashMap::new(),
            convened_accounts: HashSet::new(),
            pending_since_wall: HashMap::new(),
            next_seq: 1,
            placed: 0,
            matched: 0,
            quote_bounds: HashMap::new(),
            pair_cooldowns: HashMap::new(),
            own_sells: HashMap::new(),
            adopted: HashSet::new(),
            last_fills: HashMap::new(),
            newest_fill: HashMap::new(),
            fills_settled: 0,
            quest_reserved: HashMap::new(),
            timeout_streak: HashMap::new(),
            patience: HashMap::new(),
            gifts: Default::default(),
            auction: Default::default(),
        }
    }

    pub fn find_item(&self, item: i64) -> Option<&CatalogueItem> {
        self.catalogue.iter().find(|entry| entry.id == item)
    }

    /// A settled fill moves the current price of its item (the rings are
    /// preloaded at open, `preload_fills`).
    pub(crate) fn note_fill(&mut self, item: i64, price: i64, quantity: i64) {
        self.fills_settled += 1;
        self.newest_fill.insert(item, MarketService::now_clock());
        let ring = self.last_fills.entry(item).or_default();
        ring.push_back(LastFill { price, quantity });
        while ring.len() > PRICE_FILLS {
            ring.pop_front();
        }
    }

    pub(crate) fn remember(&mut self, print: Print) {
        self.recent.push_back(print);
        while self.recent.len() > 200 {
            self.recent.pop_front();
        }
        self.next_seq += 1;
    }
}

/// A store handle for reads (`MarketService::read_store`).
pub enum ReadStore<'a> {
    Pooled(lifescape_orch_core::store_pool::PooledStore),
    Writer(WriteGuard<'a>),
}

impl std::ops::Deref for ReadStore<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        match self {
            ReadStore::Pooled(store) => store,
            ReadStore::Writer(guard) => guard,
        }
    }
}

/// The daemon market: the single store writer plus the engine behind a lock.
pub struct MarketService {
    /// The single writer of the database (`store_writer`): writes group,
    /// reads go through `read_store` on their own connections.
    writer: Arc<StoreWriter>,
    engine: Mutex<Engine>,
    /// Items table by exact name (case-sensitive), the lowest id first:
    /// the in-memory twin of `Store::item_id_by_name` (`item_id_by_name`).
    names: std::collections::HashMap<String, i64>,
    /// Process-unique number of this service (views of its books and
    /// volumes are cached under it, `brain_market_view`).
    instance: u64,
}

impl MarketService {
    /// Process-unique number of this service.
    pub fn instance_id(&self) -> u64 {
        self.instance
    }

    /// Item id of an exact display name, the same answer as the store's
    /// `item_id_by_name` (lowest id of the name), without a read.
    pub fn item_id_by_name(&self, name: &str) -> Option<i64> {
        self.names.get(name).copied()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rebuild_candles(&self, item: i64, resolution: lifescape_store::YieldCandleResolution) -> Result<(), String> {
        self.lock_store().rebuild_candles(item, resolution).map(|_| ()).map_err(|error| error.to_string())
    }

    pub fn open(db: &std::path::Path, catalogue: Vec<CatalogueItem>) -> Result<Self, String> {
        let writer = StoreWriter::open(db).map_err(|error| error.to_string())?;
        let store = writer.write();
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
        // Plan prix lot D (user Q3 2026-11-18): each book's lot, decided
        // once. The first activation reads every book's observed unit
        // price (median of its last fills, else its game value) and
        // rescales its stored prices (`Store::set_item_lot`); later starts
        // keep the stored lots.
        if crate::market_units::lots_active() {
            let decided = !store.item_lots().map_err(|error| error.to_string())?.is_empty();
            let mut lots: Vec<(i64, i64)> = Vec::new();
            for item in catalogue.iter().filter(|item| crate::market_audit::base_of(item.id) == item.id) {
                let lot = if decided {
                    store.item_lot(item.id).map_err(|error| error.to_string())?
                } else {
                    let fills: Vec<i64> = store.last_fills(item.id, PRICE_FILLS as i64).unwrap_or_default().into_iter().map(|(price, _)| price).collect();
                    crate::market_units::first_lot(item.id, lifescape_market::median_price(&fills))
                };
                if store.set_item_lot(item.id, lot).map_err(|error| error.to_string())? {
                    crate::logln!("market: {} now quoted per {lot} units", item.name);
                }
                if lot > 1 {
                    lots.push((item.id, lot));
                }
            }
            crate::market_units::load_lots(&lots);
        } else {
            // Lots off (user 2026-11-18): every book quoted per lot comes
            // back to unit prices once (`Store::set_item_lot` rescales).
            let quoted = store.item_lots().map_err(|error| error.to_string())?;
            for (item, lot) in &quoted {
                store.set_item_lot(*item, 1).map_err(|error| error.to_string())?;
                crate::logln!("market: item {item} back to unit prices (was per {lot})");
            }
            if !quoted.is_empty() {
                crate::market_units::load_lots(&[]);
            }
        }
        // The items table as it stands (the catalogue just upserted, plus
        // any older row): `list_items` is ordered by id, so the first of a
        // name is the one `item_id_by_name` returns.
        let mut names: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for item in store.list_items().map_err(|error| error.to_string())? {
            names.entry(item.name).or_insert(item.item_id);
        }
        static INSTANCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let instance = INSTANCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        drop(store);
        let service = MarketService { writer, engine: Mutex::new(Engine::new(catalogue)), names, instance };
        // The public record is settled handovers only (match-time rows
        // from before 2026-11-13 are replaced by the settled deals).
        match service.lock_store().resync_executions_from_deals() {
            Ok(kept) => crate::logln!("market: execution log = {kept} settled handover(s)"),
            Err(error) => crate::logln!("market: execution resync failed: {error}"),
        }
        service.preload_fills()?;
        service.reload_prints();
        service.reload_books();
        Ok(service)
    }

    /// Loads the price rings (last `PRICE_FILLS` fills) and the newest
    /// fill clocks of EVERY item into the engine, two set-based reads,
    /// once at open: the price readers (`reference_lot_at`) then touch the
    /// engine only, never the store lock.
    pub fn preload_fills(&self) -> Result<(), String> {
        let (rows, clocks) = {
            let store = self.read_store();
            (
                store.last_fills_all(PRICE_FILLS as i64).map_err(|error| error.to_string())?,
                store.newest_fill_clocks().map_err(|error| error.to_string())?,
            )
        };
        let mut engine = self.lock_engine();
        engine.last_fills.clear();
        engine.newest_fill.clear();
        for (item, price, quantity) in rows {
            engine.last_fills.entry(item).or_default().push_back(LastFill { price, quantity });
        }
        engine.newest_fill.extend(clocks);
        Ok(())
    }

    /// Rebuilds the in-memory books from the persisted carnet after a
    /// restart: resting (`open`, plus partially filled `matched` rows
    /// stranded without their deals) orders rest again under their store
    /// ids, ranked by creation clock so price-time priority replays the
    /// pre-shutdown queue. Stranded non-final deals fail loudly (both
    /// parties disconnected with the daemon) and give their lots back to
    /// both sides as open orders at the deal price (neutral outcome, like
    /// a runtime `nav_down`: the in-memory relist rows died with the
    /// daemon, the carnet rows behind the match were already settled).
    /// Without this a restart orphans every order: visible in
    /// `my_orders`, invisible to matching, and the market silently dies
    /// (seen live, L11).
    pub(crate) fn reload_books(&self) {
        let clock = Self::now_clock();
        match self.lock_store().fail_stale_deals_relisting("daemon restart", clock) {
            Ok(failed) if !failed.is_empty() => {
                let ids: Vec<i64> = failed.iter().map(|deal| deal.deal_id).collect();
                crate::logln!("market reload: failed {} stale in-flight deal(s) (daemon restart), lots relisted both sides: {ids:?}", failed.len());
            }
            Ok(_) => {}
            Err(error) => crate::logln!("market reload: stale deals left as they were, store refused: {error}"),
        }
        self.rebase_note_orders();
        let rows = match self.read_store().resting_orders() {
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
            // Queue rank = creation order; arrival = last (re)entry, so a
            // carnet left crossed heals with the latest one as the taker
            // (`market_uncross`, below).
            let mut arrivals: Vec<(usize, &lifescape_store::Order)> = rows.iter().enumerate().collect();
            arrivals.sort_by_key(|(seq, row)| (row.updated_clock, *seq));
            let mut engine = self.lock_engine();
            for (seq, row) in arrivals {
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
        drop(store);
        crate::logln!(
            "market reload: {} resting orders back in {} books ({reopened} stranded matched reopened)",
            rows.len(),
            self.lock_engine().books.len()
        );
        // Recent failed pairs stay apart, then whatever still crosses
        // (old raw relists, restart relists) matches like any taker.
        self.restore_pair_cooldowns();
        let healed = self.uncross_books(crate::market_convene::wall_now());
        if !healed.is_empty() {
            crate::logln!("market reload: crossed books healed, deals {healed:?}");
        }
    }

    /// Resting orders placed on a banknote (before 2026-11-16, or relisted
    /// from a dead note deal) move to their base object's book, so both
    /// forms meet in one carnet. Idempotent, every boot.
    fn rebase_note_orders(&self) {
        let ids: Vec<i64> = self.lock_engine().catalogue.iter().map(|entry| entry.id).collect();
        let mut moved = 0usize;
        for note in ids {
            let base = crate::market_audit::book_item(note);
            if base != note && self.lock_engine().find_item(base).is_some() {
                match self.lock_store().rebase_resting_orders(note, base) {
                    Ok(count) => moved += count,
                    Err(error) => crate::logln!("market reload: note {note} orders left on their own book, store refused: {error}"),
                }
            }
        }
        if moved > 0 {
            crate::logln!("market reload: {moved} resting banknote order(s) moved to their base object's book");
        }
    }

    /// The writer connection (writes group and commit when the guard
    /// drops, `store_writer`). Read-only code uses `read_store`.
    pub fn lock_store(&self) -> WriteGuard<'_> {
        self.writer.write()
    }

    /// The writer, to register it as the process's (`store_writer::register`).
    pub fn store_writer(&self) -> &Arc<StoreWriter> {
        &self.writer
    }

    /// A connection for READS: the calling thread's pooled one, which
    /// never waits for a write. Inside a `batch` of this thread (its own
    /// uncommitted rows) or when the pool cannot open, the writer's.
    pub fn read_store(&self) -> ReadStore<'_> {
        if lifescape_orch_core::store_writer::in_batch() {
            return ReadStore::Writer(self.writer.write());
        }
        match self.writer.read() {
            Ok(store) => ReadStore::Pooled(store),
            Err(error) => {
                crate::logln!("market: read connection unavailable ({error}), reading through the writer");
                ReadStore::Writer(self.writer.write())
            }
        }
    }

    pub fn lock_engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn now_clock() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
    }

    /// One JSON-lines request in, one reply out. Pure dispatch: every
    /// op below is also covered by the service tests over fake clients.
    pub fn handle(&self, request: &Value) -> Value {
        let op = match MarketOp::of(request) {
            Ok(op) => op,
            Err(refusal) => return object(vec![("ok", Value::Bool(false)), ("error", str_value(&refusal))]),
        };
        match op {
            MarketOp::Quote => self.op_quote(request),
            MarketOp::Book => self.op_book(request),
            MarketOp::History => self.op_history(request),
            MarketOp::Trades => self.op_trades(request),
            MarketOp::List => self.op_list(request),
            MarketOp::Place => self.op_place(request),
            MarketOp::Amend => self.op_amend(request),
            MarketOp::Cancel => self.op_cancel(request),
            MarketOp::MyOrders => self.op_my_orders(request),
            MarketOp::MyDeals => self.op_my_deals(request),
            MarketOp::Recent => self.op_recent(request),
            MarketOp::DealArrived => self.op_deal_arrived(request),
            MarketOp::DealReport => self.op_deal_report(request),
            MarketOp::Pending => self.op_pending(request),
            MarketOp::Convene => self.op_convene(request),
            MarketOp::Sweep => self.op_sweep(request),
            MarketOp::Fund => self.op_fund(request),
            MarketOp::Tick => self.op_tick(request),
            MarketOp::Stats => self.op_stats(),
            MarketOp::SellerQuote => crate::market_facts::seller_quote(self, request),
        }
    }

    pub fn err(message: String) -> Value {
        object(vec![("ok", Value::Bool(false)), ("error", str_value(&message))])
    }

    fn op_history(&self, request: &Value) -> Value {
        // One book per object: a banknote asks about its object.
        let item = request.get("item").as_i64().map(crate::market_audit::book_item).unwrap_or(-1);
        if self.lock_engine().find_item(item).is_none() {
            return Self::err(format!("unknown item {item}"));
        }
        // `15m` (24 h view), `1h` (7 days), `1d`; unknown reads hourly.
        let resolution = YieldCandleResolution::from_code(request.get("res").as_str().unwrap_or("1h"))
            .unwrap_or(YieldCandleResolution::Hour1);
        // Half-open store windows (`clock < to`) would drop prints from
        // the current second, so the live edge sits one second out.
        let to = request.get("to").as_i64().unwrap_or_else(|| Self::now_clock() + 1);
        let default_from = match resolution {
            YieldCandleResolution::Min15 | YieldCandleResolution::Hour1 => to - 86400,
            YieldCandleResolution::Day1 => to - 183 * 86400,
        };
        let from = request.get("from").as_i64().unwrap_or(default_from);
        // No gap (user rule 2026-11-13): candles open at the previous
        // close and trade-less windows are flat at the last close, up to
        // the live window (`to` capped at now: the web bridge ceils it).
        let live_edge = to.min(Self::now_clock() + 1);
        // The writer only for the candles (a missing cache is rebuilt, a
        // write); the plain reads and the JSON go on a pooled connection
        // with no writer lock (lot 5.8 correction).
        let (candles, previous_close) = {
            let mut store = self.lock_store();
            store.continuous_candles(item, resolution, from, live_edge).unwrap_or_default()
        };
        let store = self.read_store();
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
                    ("vb", Value::Int(candle.buy_volume)),
                    ("vs", Value::Int(candle.sell_volume)),
                    ("n", Value::Int(count)),
                ])
            })
            .collect();
        // The prints with their parties (user 2026-11-18): an execution is
        // its settled deal's (same item, clock, quantity and price).
        let mut parties: Vec<(i64, i64, i64, String, String)> = store
            .settled_deals(Some(item), 100)
            .unwrap_or_default()
            .into_iter()
            .map(|deal| (deal.updated_clock, deal.quantity, deal.price, deal.buyer, deal.seller))
            .collect();
        let tail: Vec<Value> = executions
            .iter()
            .rev()
            .take(30)
            .map(|exec| {
                let found = parties.iter().position(|(clock, quantity, price, _, _)| *clock == exec.clock && *quantity == exec.quantity && *price == exec.price);
                let (buyer, seller) = found.map(|at| parties.remove(at)).map(|(_, _, _, buyer, seller)| (buyer, seller)).unwrap_or_default();
                crate::market_prints::print_value(&Print { item, quantity: exec.quantity, price: exec.price, clock: exec.clock, taker: exec.taker_side, buyer, seller })
            })
            .collect();
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("res", str_value(resolution.as_str())),
            ("lot", Value::Int(crate::market_units::lot_of(item))),
            ("candles", Value::Arr(candle_values)),
            ("trades", Value::Arr(tail)),
            ("prev_close", previous_close.map(Value::Int).unwrap_or(Value::Null)),
        ])
    }

    fn op_trades(&self, request: &Value) -> Value {
        // One book per object: a banknote asks about its object.
        let item = request.get("item").as_i64().map(crate::market_audit::book_item).unwrap_or(-1);
        let limit = request.get("limit").as_i64().unwrap_or(20).clamp(1, 100) as usize;
        let engine = self.lock_engine();
        let mut prints: Vec<&Print> = engine.recent.iter().filter(|print| item < 0 || print.item == item).collect();
        let total = prints.len();
        prints = prints.into_iter().skip(total.saturating_sub(limit)).collect();
        let values: Vec<Value> = prints.iter().map(|print| crate::market_prints::print_value(print)).collect();
        object(vec![("ok", Value::Bool(true)), ("trades", Value::Arr(values)), ("seq", Value::Int(engine.next_seq as i64))])
    }

    fn op_list(&self, request: &Value) -> Value {
        let query = request.get("q").as_str().unwrap_or("").to_lowercase();
        let limit = request.get("limit").as_i64().unwrap_or(200).clamp(1, 2000) as usize;
        let traded_only = request.get("traded_only").as_i64().unwrap_or(0) == 1;
        // `with_orders=1`: keep goods with >= 1 open order (either side).
        let with_orders = request.get("with_orders").as_i64().unwrap_or(0) == 1;
        // `active=1`: keep goods with >= 1 open order OR >= 1 execution ever
        // (market page, user rule 2026-11-13: a traded good stays listed
        // once its book empties; never-traded empty books stay hidden).
        let active = request.get("active").as_i64().unwrap_or(0) == 1;
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
        let traded: std::collections::HashSet<i64> = if traded_only || active {
            self.read_store().items_with_executions().unwrap_or_default().into_iter().collect()
        } else { std::collections::HashSet::new() };
        // One row per named good: 24h volume summed over every form,
        // change (display only) from the merged fills, canonical id = the
        // form carrying the volume (base on ties). Sorted by descending
        // volume so the traded goods surface first.
        let store = self.read_store();
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
            if active && order_count == 0 && !members.iter().any(|member| traded.contains(&member.0)) {
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
            // The day's first trade: the ticker's base for an object
            // first traded within the day.
            let window_open = fills.iter().find(|fill| fill.quantity > 0 && fill.price >= 1).map(|fill| fill.price);
            let forms: Vec<i64> = volumes.iter().map(|member| member.0).collect();
            items.push((name.clone(), canonical, forms, window_open, volume, order_count, open_qty));
        }
        drop(store);
        // Descending 24h volume, then the largest open quantity, then the name.
        items.sort_by(|left, right| {
            right.4.cmp(&left.4).then(right.6.cmp(&left.6)).then(left.0.cmp(&right.0)).then(left.1.cmp(&right.1))
        });
        let rows: Vec<Value> = items
            .into_iter()
            .take(limit)
            .map(|(name, id, forms, window_open, volume, order_count, open_qty)| {
                // The form that traded; the current price bots read (last
                // five fills clamped by its book) rides as `ref`.
                let priced = forms.iter().copied().find(|form| !self.last_fill_prices(*form).is_empty()).unwrap_or(id);
                // Per lot like the book and the prints (plan prix lot D).
                let reference = self.reference_lot(priced).price;
                // Displayed like an exchange ticker (user 2026-11-18): the
                // last trade, its change against the price 24 h ago.
                let (last, day_ago) = {
                    let store = self.read_store();
                    (store.last_price_before(priced, clock + 1).ok().flatten(), store.last_price_before(priced, clock - 86_400 + 1).ok().flatten())
                };
                let change = lifescape_market::ticker_change_thousandths(last, day_ago, window_open);
                object(vec![
                    ("orders", Value::Int(order_count as i64)),
                    ("open_qty", Value::Int(open_qty)),
                    ("item", Value::Int(id)),
                    ("name", str_value(&name)),
                    ("price", Value::Int(last.unwrap_or(reference))),
                    ("ref", Value::Int(reference)),
                    ("lot", Value::Int(crate::market_units::lot_of(priced))),
                    ("change24h", Value::Int(change)),
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
        // AGENTS rule 13: staff cheated goods and coins never reach
        // another account, so admin and admin2 never trade on the market.
        if crate::economy_guard::is_staff(&account) {
            return Self::err(format!("{account} never trades: staff cheated goods stay out of the economy (rule 13)"));
        }
        let side = match Self::side_of(request) {
            Ok(side) => side,
            Err(error) => return Self::err(error),
        };
        // A banknote rests in its base object's book (one book per object).
        let item = crate::market_audit::book_item(request.get("item").as_i64().unwrap_or(-1));
        // Plan prix lot D: a caller prices per unit (the brain, the
        // scripts) unless it says `per_lot` (the repricing, the market's
        // own walks); the book holds coins per lot, whole lots only (a
        // listing rounds down, a purchase up to the lot covering it).
        let per_lot = request.get("per_lot").as_bool() == Some(true);
        let lot = crate::market_units::lot_of(item);
        let asked = request.get("qty").as_i64().unwrap_or(0);
        let qty = if side == Side::Sell { lifescape_market::lots::whole_lots_down(asked, lot) } else { lifescape_market::lots::whole_lots_up(asked, lot) };
        if asked > 0 && qty == 0 {
            return Self::err(format!("{asked} units of {item} are under one lot of {lot}"));
        }
        let raw_price = request.get("price").as_i64().unwrap_or(0);
        let price = if per_lot { raw_price } else { lifescape_market::lots::lot_price(raw_price, lot) };
        if self.lock_engine().find_item(item).is_none() {
            return Self::err(format!("unknown item {item}"));
        }
        if let Err(error) = check_price(price) {
            return Self::err(error.to_string());
        }
        // Why the order exists (plan marché M1): kept by its amends.
        let motive = request.get("motive").as_str().unwrap_or("").to_string();
        if let Err(error) = crate::market_lineage::check_motive(&motive) {
            return Self::err(error);
        }
        {
            // Net of the account's resting orders and in-flight deals
            // (`market_audit::uncommitted`): promised lots are never
            // offered twice (deals 1716/1721/1772, 2026-11-13).
            let (have_item, have_coins) = self.free_coverage(&account, Self::order_side(side), item);
            if let Err(error) = check_coverage(side, qty, lifescape_market::lots::lot_coins(qty, price, lot), have_item, have_coins) {
                return Self::err(error.to_string());
            }
        }
        let clock = Self::now_clock();
        // Lot 3 marché: a pressed order (blocking need, urgent sale) on an
        // auction object is served by an auction at once (below), and the
        // mode of an object seen for the first time is fixed before the
        // order can match (`market_auction`).
        let pressed = (motive == crate::market_lineage::NEED || request.get("urgent").as_bool() == Some(true), motive.clone());
        crate::market_auction::ensure_mode(self, item);
        // The life's drawn trait, never the store's 0.5 (plan M3).
        let price_patience = crate::market_reprice::account_patience(self, &account);
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
                    price_patience,
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
                replaces: None,
                motive, ceiling: crate::market_reprice::request_ceiling(request, side == Side::Sell, if per_lot { 1 } else { lot }),
                ask_limit: crate::market_reprice::request_ask_limit(request, side == Side::Sell, if per_lot { 1 } else { lot }),
            }) {
                Ok(id) => id,
                Err(error) => return Self::err(error.to_string()),
            }
        };
        let order_key = store_id as u64;
        let placed = {
            let mut guard = self.lock_engine();
            let engine: &mut Engine = &mut guard;
            // Failed-deal pairs stay apart (`market_uncross`).
            let kept_apart = crate::market_uncross::kept_apart(&engine.pair_cooldowns, crate::market_convene::wall_now());
            let book = engine.books.entry(item).or_insert_with(Book::new);
            // Pre-match snapshot for the priority-preserving relist: the
            // resting rows keep their `seq`, the taker its arrival rank.
            let pre: HashMap<u64, Order> = book.orders().into_iter().map(|order| (order.id, order)).collect();
            let taker_seq = book.next_seq();
            match book.place_guarded(order_key, &account, item, side, qty, price, &kept_apart) {
                Ok((fills, rests)) => {
                    let matched: Vec<(lifescape_market::Fill, Vec<Order>)> = fills
                        .into_iter()
                        .map(|fill| {
                            let rows =
                                crate::market_convene::relist_for_fill(&pre, taker_seq, side, &account, price, &fill);
                            (fill, rows)
                        })
                        .collect();
                    Ok((matched, rests))
                }
                Err(error) => Err(error.to_string()),
            }
        };
        // The store is touched only once the engine guard is gone.
        let (matched, rests) = match placed {
            Ok(outcome) => outcome,
            Err(error) => {
                self.lock_store().cancel_order(store_id, clock).ok();
                return Self::err(error);
            }
        };
        let fills: Vec<lifescape_market::Fill> = matched.iter().map(|(fill, _)| fill.clone()).collect();
        // One deal per counterparty and price (the book rows above keep
        // their per-fill remaining; `market_merge`).
        let matched = crate::market_merge::merge_fills(matched);
        self.lock_engine().order_item.insert(order_key, item);
        if rests {
            if let Some(bounds) = crate::market_reprice::bounds_from_request(request, &account, side == Side::Sell, price, store_id, clock, if per_lot { 1 } else { lot }) {
                crate::market_reprice::register(self, store_id, bounds);
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
        // A pressed order on an auction object: the auction runs now.
        let mut rests = rests;
        if rests && pressed.0 {
            let served = self.pressed_auction(item, &pressed.1, true);
            if !served.is_empty() {
                let mine = self.read_store().open_deals_for_account(&account).unwrap_or_default();
                deal_ids.extend(mine.iter().filter(|deal| served.contains(&deal.deal_id)).map(|deal| Value::Int(deal.deal_id)));
                rests = self.lock_engine().books.get(&item).is_some_and(|book| book.remaining(order_key).is_some());
            }
        }
        let status = if rests { "open" } else { "filled" };
        object(vec![
            ("ok", Value::Bool(true)),
            ("order", Value::Int(store_id)),
            ("status", str_value(status)),
            ("lot", Value::Int(lot)),
            ("qty", Value::Int(qty)),
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

    fn op_my_orders(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let store = self.read_store();
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
        let fresh: Vec<Value> =
            engine.recent.iter().filter(|print| print.clock > since).map(crate::market_prints::print_value).collect();
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
