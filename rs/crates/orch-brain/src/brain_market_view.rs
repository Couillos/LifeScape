#![forbid(unsafe_code)]
//! The market as every bot reads it, built once per change instead of once
//! per bot and pass (lot 3.2 of the architecture plan; analysis 2.2 (c)).
//!
//! `brain_bids::MarketReader::live` used to copy every resting order of
//! every book for each bot (`wait_answer` 868 ms, `bids.goal_for` 237 ms
//! per call on the 74-bot fleet) and ran one store query per candidate
//! object for the 24 h volume. Two shared, numbered views replace that:
//!
//! - `SharedBooks`: every book's bids (with their owner) and asks, built
//!   under the engine lock and published as an `Arc`. Its generation is
//!   the number of books plus the sum of their change counters
//!   (`Book::version`, moved by every call that may change a resting
//!   order): the view is current while the generation is. A bot's own
//!   bids are left out when ITS view is read (`ItemBook::view_for`), never
//!   in the shared copy.
//! - `fills`: the executions of one object over the last day, copied once
//!   per `Store::executions_stamp` (the log only grows) and summed for the
//!   window at the clock of the call, so the sliding 24 h window is exact
//!   at any clock, not the volume of the moment the copy was made.
//!
//! Both are keyed by the service instance (`MarketService::instance_id`).
//! Neutral: the views equal the old per-bot computation at equal book
//! (`brain_market_view_tests`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use lifescape_orch_market::market_service::MarketService;
use lifescape_planner::market_fit::BookView;

/// One book as the engine holds it: every resting order with units left.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ItemBook {
    /// `(owner, price per unit, remaining)`, best price first (ties in the
    /// order the book lists them).
    bids: Vec<(String, i64, i64)>,
    /// `(price per unit, remaining)`, cheapest first.
    asks: Vec<(i64, i64)>,
}

impl ItemBook {
    /// The view `account` reads: its own bids left out, its own asks kept
    /// (supply like any other).
    pub(crate) fn view_for(&self, account: &str) -> BookView {
        BookView {
            bids: self.bids.iter().filter(|(owner, _, _)| !owner.eq_ignore_ascii_case(account)).map(|(_, price, remaining)| (*price, *remaining)).collect(),
            asks: self.asks.clone(),
            ..BookView::default()
        }
    }

    /// True when some account other than `account` bids here.
    pub(crate) fn has_bid_besides(&self, account: &str) -> bool {
        self.bids.iter().any(|(owner, _, _)| !owner.eq_ignore_ascii_case(account))
    }
}

/// Every object's public book, numbered by generation.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct SharedBooks {
    generation: u64,
    /// Market id of every object that holds a book (the base object's id,
    /// never its banknote's).
    ids: HashMap<String, i64>,
    items: HashMap<String, ItemBook>,
}

impl SharedBooks {
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn ids(&self) -> &HashMap<String, i64> {
        &self.ids
    }

    pub(crate) fn item(&self, name: &str) -> Option<&ItemBook> {
        self.items.get(name)
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &String> {
        self.items.keys()
    }

    /// Builds the views from `(item debug name, id of the book, owner, buy?,
    /// price per unit, remaining)` rows in book order (the pure part,
    /// shared with the tests' reference).
    pub(crate) fn from_rows(generation: u64, ids: HashMap<String, i64>, rows: &[(String, String, bool, i64, i64)]) -> Self {
        let mut items: HashMap<String, ItemBook> = HashMap::new();
        for (item, owner, buy, price, remaining) in rows.iter().filter(|row| row.4 > 0) {
            let book = items.entry(item.clone()).or_default();
            if *buy {
                book.bids.push((owner.clone(), *price, *remaining));
            } else {
                book.asks.push((*price, *remaining));
            }
        }
        for book in items.values_mut() {
            book.bids.sort_by(|left, right| right.1.cmp(&left.1));
            book.asks.sort_by_key(|(price, _)| *price);
        }
        SharedBooks { generation, ids, items }
    }
}

/// Debug name of a market object (banknotes trade as their base).
fn debug_name(item_id: i64) -> Option<String> {
    let obj = lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?)?;
    let name = obj.name?;
    Some(name.strip_prefix("cert_").unwrap_or(name).to_string())
}

/// Id of the base (non-banknote) object named `name`.
fn base_id(name: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// The books' generation under the engine lock.
fn generation_of(books: &HashMap<i64, lifescape_market::Book>) -> u64 {
    books.values().fold(books.len() as u64, |sum, book| sum.wrapping_add(book.version()))
}

/// The generation of the live books (`SharedBooks::generation` of a copy
/// built now), without building the copy.
pub(crate) fn books_generation(service: &MarketService) -> u64 {
    generation_of(&service.lock_engine().books)
}

/// Reads the rows of every book in a fixed order (by object id: the old
/// read walked a hash map, so the order of rows of equal price was never
/// specified).
pub(crate) fn rows_of(service: &MarketService) -> (u64, HashMap<String, i64>, Vec<(String, String, bool, i64, i64)>) {
    let engine = service.lock_engine();
    let generation = generation_of(&engine.books);
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut rows = Vec::new();
    let mut book_ids: Vec<&i64> = engine.books.keys().collect();
    book_ids.sort_unstable();
    for item_id in book_ids {
        let Some(book) = engine.books.get(item_id) else { continue };
        let Some(name) = debug_name(*item_id) else { continue };
        // The base object's id, never its banknote's (both forms may hold a
        // book): the current price reads the base reference.
        ids.entry(name.clone()).or_insert_with(|| base_id(&name).unwrap_or(*item_id));
        for order in book.orders() {
            rows.push((name.clone(), order.account.clone(), order.side == lifescape_market::Side::Buy, crate::market_units::to_unit(*item_id, order.price), order.remaining));
        }
    }
    (generation, ids, rows)
}

fn books_cache() -> &'static Mutex<Option<(u64, Arc<SharedBooks>)>> {
    static CACHE: OnceLock<Mutex<Option<(u64, Arc<SharedBooks>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The books as they stand: the published copy when its generation is the
/// engine's, else a copy built now (one pass over the books, shared by
/// every bot until the next change).
pub(crate) fn shared_books(service: &MarketService) -> Arc<SharedBooks> {
    let generation = {
        let engine = service.lock_engine();
        generation_of(&engine.books)
    };
    {
        let cache = books_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((instance, published)) = cache.as_ref() {
            if *instance == service.instance_id() && published.generation == generation {
                return published.clone();
            }
        }
    }
    let (generation, ids, rows) = rows_of(service);
    let built = Arc::new(SharedBooks::from_rows(generation, ids, &rows));
    *books_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((service.instance_id(), built.clone()));
    built
}

/// One object's executions over a day, copied once per log stamp.
struct FillsCopy {
    instance: u64,
    stamp: i64,
    /// First clock the copy holds (the window start of the clock it was
    /// copied at): a later clock needs no older row.
    from: i64,
    rows: Vec<(i64, i64)>,
}

fn fills_cache() -> &'static Mutex<HashMap<i64, FillsCopy>> {
    static CACHE: OnceLock<Mutex<HashMap<i64, FillsCopy>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const DAY: i64 = 86_400;

/// Units of the object `item_id` traded in `[clock - 24 h, clock]`:
/// `Store::volume_between(item_id, clock - 86400, clock + 1)`, summed from
/// the copy of the day's executions.
pub(crate) fn volume_24h_of(service: &MarketService, item_id: i64, clock: i64) -> i64 {
    let store = service.read_store();
    let Ok(stamp) = store.executions_stamp() else { return store.volume_between(item_id, clock - DAY, clock + 1).unwrap_or(0) };
    let window = clock - DAY;
    let mut cache = fills_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let current = cache.get(&item_id).filter(|copy| copy.instance == service.instance_id() && copy.stamp == stamp && copy.from <= window);
    if current.is_none() {
        match store.fills_since(item_id, window) {
            Ok(rows) => {
                cache.insert(item_id, FillsCopy { instance: service.instance_id(), stamp, from: window, rows });
            }
            Err(_) => return store.volume_between(item_id, window, clock + 1).unwrap_or(0),
        }
    }
    cache.get(&item_id).map_or(0, |copy| copy.rows.iter().filter(|(at, _)| *at >= window && *at < clock + 1).map(|(_, quantity)| *quantity).sum())
}

#[cfg(test)]
#[path = "brain_market_view_tests.rs"]
mod tests;
