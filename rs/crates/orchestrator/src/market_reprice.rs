#![forbid(unsafe_code)]
//! Order repricing (plan marché M3/M4, wave 2; the 10 % walk, user
//! 2026-11-19: « la règle des 10 % vaut pour tout le monde en bid et ask,
//! sinon les prix ne sont pas réels »). Each window (its length from the
//! motive's horizon, its phase from the account and object: no walk in a
//! block) one `concession::step`:
//! - an unfilled ask drops 10 % (at least a coin) down to one coin, under
//!   its own cost (no seller floor, never withdrawn); an unfilled bid
//!   climbs 10 % (at least a coin) up to its ceiling (what the buyer can
//!   pay);
//! - an own fill backs the price off one ZIP step (a seller up, a buyer
//!   down);
//! - a public traded price on the conceding side pulls the quote halfway
//!   toward it (public data only);
//! - a pressed order (blocking need, full bank) takes the opposite best
//!   inside its limit; any order takes what its next walk reaches.
//!
//! Bounds live in the market engine's memory: after a daemon restart the
//! brain re-adopts its resting listings (`adopt`) and its standing bids
//! (`keep_bid`), lineage and t0 read back from the carnet.

use std::collections::HashMap;

use lifescape_json::{object, str_value, Value};
use lifescape_market::concession::{self, Concession, Step, Urgency, Window};

use crate::market_service::MarketService;

/// Old fixed window (ticks): the "fast fill" of
/// `lifescape_market::OwnFill` is a fill inside this first window.
pub const PATIENCE_TICKS: u64 = lifescape_market::zip::PATIENCE_TICKS;
/// A sell's own cap: none (no reference band, user rule 2026-11-15).
pub const SELL_CAP: i64 = i64::MAX / 4;

/// Repricing bounds of one resting order.
#[derive(Clone, Debug)]
pub struct QuoteBounds {
    pub account: String,
    /// Object of the order (an auction object's orders walk only after
    /// its auction, `market_auction`).
    pub item: i64,
    pub sell: bool,
    pub floor: i64,
    pub ceiling: i64,
    /// Ticks of one window (`concession::window_ticks` of the horizon,
    /// or the request's `patience`).
    pub patience_ticks: u64,
    /// Daemon wall seconds of the last window close.
    pub last_change_wall: f64,
    /// Units still open when the window started: fewer now = it filled.
    pub last_remaining: i64,
    /// Daemon wall seconds the listing went up (kept across its walk
    /// steps): how fast a fill came (`market_seller::remember_fill`).
    pub listed_wall: f64,
    /// Lineage of the order (plan marché M1): its first order and t0
    /// (store clock seconds), kept by every amend and read back from the
    /// carnet after a restart (`for_row`).
    pub root_id: i64,
    pub root_clock: i64,
    /// Why the order exists (`market_lineage` motives) and its horizon in
    /// ticks (§3.5): the concession reaches its limit there.
    pub motive: String,
    pub horizon_ticks: u64,
    /// Opening price of the curve (the lineage's, or the ZIP back-off of
    /// its last own fill).
    pub opening: i64,
    /// Wall seconds the curve started (lineage t0, or the last own fill).
    pub curve_start_wall: f64,
    /// The bot's drawn price patience (`None` until the first window
    /// reads it from its life, `account_patience`).
    pub price_patience: Option<f64>,
    /// Pressed beyond its motive (a seller with a full bank, D17).
    pub pressed: bool,
    /// Price of its last own fill (its public print is never a pull).
    pub own_fill: Option<i64>,
}

impl QuoteBounds {
    /// Bounds of a resting carnet row: lineage, t0 and motive read from
    /// the row (adoption after a daemon restart, a kept standing bid),
    /// never restarted at the adoption. The window phase spreads the
    /// adopted orders (never one block after a restart, plan C6).
    pub fn for_row(row: &lifescape_store::Order, floor: i64, ceiling: i64) -> QuoteBounds {
        let sell = row.side == lifescape_store::OrderSide::Sell;
        let wall = wall_now();
        let horizon_ticks = crate::market_lineage::horizon_ticks(&row.motive, sell);
        let root_clock = row.lineage_clock();
        let window = concession::window_ticks(horizon_ticks);
        QuoteBounds {
            account: row.account.clone(),
            item: row.item_id,
            sell,
            floor,
            ceiling,
            patience_ticks: window,
            last_change_wall: wall - phase_secs(&row.account, row.item_id, window),
            last_remaining: row.remaining,
            listed_wall: wall,
            root_id: if row.root_id > 0 { row.root_id } else { row.order_id },
            root_clock,
            motive: row.motive.clone(),
            horizon_ticks,
            opening: row.price,
            // The curve walks from the price it rests at now, over what is
            // left of the lineage's horizon (t counts from t0).
            curve_start_wall: root_clock as f64,
            price_patience: None,
            pressed: false,
            own_fill: None,
        }
    }

    /// The order's own limit: a seller's floor, a buyer's ceiling.
    pub fn limit(&self) -> i64 {
        if self.sell {
            self.floor.max(1)
        } else {
            self.ceiling.max(1)
        }
    }

    /// Urgency of the motive (blocking need: pressed; stock bought ahead
    /// and loot: patient), or pressed by state (`pressed`).
    pub fn urgency(&self) -> Urgency {
        if self.pressed {
            return Urgency::Pressed;
        }
        match self.motive.as_str() {
            crate::market_lineage::NEED => Urgency::Pressed,
            crate::market_lineage::STOCK | crate::market_lineage::LOOT => Urgency::Patient,
            _ => Urgency::Normal,
        }
    }
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// Window phase of an order in wall seconds (at the measured tick).
fn phase_secs(account: &str, item: i64, window: u64) -> f64 {
    let tick_ms = crate::market_eta::live_tick_ms().max(1) as f64;
    concession::window_offset(account, item, window) as f64 * tick_ms / 1000.0
}

/// Registers bounds for a resting order (called by `place`).
pub fn register(service: &MarketService, order: i64, bounds: QuoteBounds) {
    service.lock_engine().quote_bounds.insert(order, bounds);
}

/// Bounds of a resting order, if it reprices.
pub fn bounds_of(service: &MarketService, order: i64) -> Option<QuoteBounds> {
    service.lock_engine().quote_bounds.get(&order).cloned()
}

/// The account's drawn `price_patience` (its life's profile trait, plan
/// M3), read once per daemon life; 0.5 when the account has no life (a
/// manual bot, a test account).
pub fn account_patience(service: &MarketService, account: &str) -> f64 {
    if let Some(known) = service.lock_engine().patience.get(account).copied() {
        return known;
    }
    // Tests never read the real run/lives (a fleet account's own trait
    // would change the curve under test): they inject into the cache.
    let patience = if cfg!(test) { 0.5 } else { crate::brain::load_life(account).map(|life| life.profile.price_patience).unwrap_or(0.5) };
    service.lock_engine().patience.insert(account.to_string(), patience);
    patience
}

/// Bounds parsed from a `place` request: `ceiling` (buy: the most it
/// pays; sell: an optional cap) or `floor` (sell reserve), optional
/// `patience` window ticks, `motive`, `urgent` (a full bank). `None` =
/// fixed-price order. The new order `order` (placed at store clock
/// `clock`) is its lineage's root and opens the curve.
pub fn bounds_from_request(request: &Value, account: &str, sell: bool, price: i64, order: i64, clock: i64, lot: i64) -> Option<QuoteBounds> {
    // `price` is the book's (per lot); the request's floor and ceiling
    // are the caller's (per unit unless it priced per lot: `lot` 1).
    let ceiling = request.get("ceiling").as_i64().map(|unit| lifescape_market::lots::lot_price(unit, lot));
    let floor = request.get("floor").as_i64().map(|unit| lifescape_market::lots::lot_price(unit, lot));
    if ceiling.is_none() && floor.is_none() {
        return None;
    }
    let (floor, ceiling) = if sell {
        (floor.unwrap_or(1).clamp(1, price.max(1)), ceiling.unwrap_or(SELL_CAP).max(price))
    } else {
        (price, ceiling.unwrap_or(price).max(price))
    };
    let motive = request.get("motive").as_str().unwrap_or("").to_string();
    let horizon_ticks = crate::market_lineage::horizon_ticks(&motive, sell);
    let item = request.get("item").as_i64().unwrap_or(0);
    let window = request.get("patience").as_i64().map(|ticks| ticks.max(1) as u64).unwrap_or_else(|| concession::window_ticks(horizon_ticks));
    let wall = wall_now();
    Some(QuoteBounds {
        account: account.to_string(),
        item,
        sell,
        floor,
        ceiling,
        patience_ticks: window,
        last_change_wall: wall - phase_secs(account, item, window),
        last_remaining: request.get("qty").as_i64().unwrap_or(0),
        listed_wall: wall,
        root_id: order,
        root_clock: clock,
        motive,
        horizon_ticks,
        opening: price,
        curve_start_wall: wall,
        price_patience: None,
        pressed: request.get("urgent").as_bool() == Some(true),
        own_fill: None,
    })
}

fn resting(row: &lifescape_store::Order) -> bool {
    matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0
}

/// The concession window of one resting order (pure): what it sees at
/// daemon wall `wall` with the measured tick `tick_ms`.
#[allow(clippy::too_many_arguments)]
pub fn window_of(bounds: &QuoteBounds, row_price: i64, filled: bool, wall: f64, tick_ms: f64, anchor: Option<i64>, best_opposite: Option<i64>, patience: f64) -> Window {
    let elapsed = ((wall - bounds.curve_start_wall).max(0.0) * 1000.0 / tick_ms.max(1.0)) as u64;
    Window {
        sell: bounds.sell,
        current: row_price,
        curve: Concession {
            opening: bounds.opening.max(1),
            limit: bounds.limit(),
            horizon_ticks: bounds.horizon_ticks,
            beta: concession::concession_beta(patience, bounds.urgency()),
        },
        elapsed_ticks: elapsed,
        window_ticks: bounds.patience_ticks,
        filled,
        anchor_price: anchor,
        own_fill: bounds.own_fill,
        best_opposite,
        pressed: bounds.urgency() == Urgency::Pressed,
    }
}

/// One repricing pass at daemon wall `wall` with the measured tick
/// length. Returns the number of orders moved. Work follows change, not
/// the order count (2026-11-14, daemon CPU): an order inside its window
/// is skipped before any store read; a hold closes its window without
/// an amend; the amends of a pass share one store transaction and the
/// pass logs one summary line.
pub fn reprice_round(service: &MarketService, wall: f64, tick_ms: f64) -> usize {
    // B2: an auction object's orders do not walk between two auctions;
    // after one, the orders it left unserved walk once (`pending`).
    let (auctioned, pending) = {
        let mut engine = service.lock_engine();
        (engine.auction.items.clone(), std::mem::take(&mut engine.auction.walk_items))
    };
    let due: Vec<(i64, QuoteBounds)> = service
        .lock_engine()
        .quote_bounds
        .iter()
        .filter(|(_, bounds)| {
            if auctioned.contains(&bounds.item) {
                pending.contains(&bounds.item)
            } else {
                wall - bounds.last_change_wall >= bounds.patience_ticks as f64 * tick_ms.max(1.0) / 1000.0
            }
        })
        .map(|(order, bounds)| (*order, bounds.clone()))
        .collect();
    if due.is_empty() {
        return 0;
    }
    let batch = service.lock_store().begin_batch().unwrap_or(false);
    let (mut moved, mut parked, mut refused) = (0usize, 0usize, 0usize);
    let mut anchors: HashMap<(i64, String), Option<i64>> = HashMap::new();
    for (order, mut bounds) in due {
        let row = service.lock_store().get_order(order).ok().flatten();
        let Some(row) = row.filter(resting) else {
            service.lock_engine().quote_bounds.remove(&order);
            continue;
        };
        // Plan prix §6: out of the book (matched, its deal in delivery),
        // the order waits for its deal; an amend would only be refused.
        if !service.lock_engine().books.get(&row.item_id).is_some_and(|book| book.remaining(order as u64).is_some()) {
            service.lock_engine().quote_bounds.insert(order, QuoteBounds { last_change_wall: wall, ..bounds });
            parked += 1;
            continue;
        }
        let filled = row.remaining < bounds.last_remaining;
        let patience = match bounds.price_patience {
            Some(known) => known,
            None => account_patience(service, &bounds.account),
        };
        bounds.price_patience = Some(patience);
        // B4: the volume-weighted mean of the item's recent settled
        // trades, the asking account's own fills left out.
        let traded = *anchors.entry((row.item_id, bounds.account.clone())).or_insert_with(|| crate::market_auction::anchor_of(service, row.item_id, &bounds.account, tick_ms));
        let (bid, ask) = service.lock_engine().books.get(&row.item_id).map(|book| book.best()).unwrap_or((None, None));
        let opposite = if bounds.sell { bid } else { ask };
        let decision = concession::step(&window_of(&bounds, row.price, filled, wall, tick_ms, traded, opposite, patience));
        let mut window = QuoteBounds { last_change_wall: wall, last_remaining: row.remaining, ..bounds };
        // A bid rises only as far as the free coins go (user 2026-11-19,
        // point 1): the walk never promises more coins than the wallet.
        let selling = window.sell;
        let unsaid = window.ceiling >= UNSAID_CEILING;
        let afford = |price: i64| {
            if selling || price <= row.price {
                return price;
            }
            let (_, free) = service.free_coverage(&row.account, lifescape_store::OrderSide::Buy, row.item_id);
            crate::market_audit::affordable_raise(row.item_id, row.remaining, row.price, price, unsaid_room(unsaid, free, crate::market_units::deal_coins(row.item_id, row.remaining, row.price)))
        };
        let next = match decision {
            Step::Hold => None,
            Step::Amend(price) => Some(afford(price)),
            Step::Restart { opening } => {
                window.opening = opening;
                window.curve_start_wall = wall;
                window.own_fill = Some(row.price);
                Some(opening)
            }
        };
        let Some(next) = next.filter(|price| *price != row.price) else {
            // A hold: the window closes, no amend, no log.
            service.lock_engine().quote_bounds.insert(order, window);
            parked += 1;
            continue;
        };
        let reply = service.handle(&object(vec![
            ("op", str_value("amend")),
            ("account", str_value(&window.account)),
            ("order", Value::Int(order)),
            ("price", Value::Int(next)),
            ("per_lot", Value::Bool(true)),
        ]));
        let mut engine = service.lock_engine();
        let guard = &mut engine.quote_bounds;
        if reply.get("ok").as_bool() == Some(true) {
            guard.remove(&order);
            if let Some(replacement) = reply.get("order").as_i64() {
                guard.insert(replacement, window);
            }
            moved += 1;
        } else {
            // The original rests untouched (the checks run before the
            // cancel): it keeps walking next window.
            guard.insert(order, window);
            refused += 1;
        }
    }
    if batch {
        if let Err(error) = service.lock_store().commit_batch() {
            crate::logln!("market-reprice: batch commit failed: {error}");
        }
    }
    if moved + refused > 0 {
        crate::logln!("market-reprice: {moved} moved, {parked} held, {refused} refused");
    }
    moved
}

/// Per-lot ceiling a place request states for a buy (schema 10: kept
/// with the order, the walk survives a restart); none for a sell.
pub fn request_ceiling(request: &Value, sell: bool, lot: i64) -> Option<i64> {
    if sell {
        return None;
    }
    request.get("ceiling").as_i64().map(|unit| lifescape_market::lots::lot_price(unit.max(1), lot))
}

/// Ceiling of an adopted bid that never stated one: what the buyer can
/// pay, each raise capped by its free coins (`affordable_raise`) and the
/// whole order by `UNSAID_SHARE_PCT` of them (`unsaid_room`).
pub const UNSAID_CEILING: i64 = i64::MAX / 4;
/// Share of the buyer's coins (free plus this order's) an orphan bid
/// may engage (live 2026-11-20: eva_dale's nature rune bid climbed to
/// 1 853 a rune, 122 298 coins, her free coins 0, every other bid
/// refused).
pub const UNSAID_SHARE_PCT: i64 = 10;

/// Coins a raise may add to an order: all the free coins for a bid with
/// a ceiling; for an orphan bid (`unsaid`), what keeps the order within
/// `UNSAID_SHARE_PCT` of the free coins plus its own.
pub fn unsaid_room(unsaid: bool, free: i64, order_coins: i64) -> i64 {
    if !unsaid {
        return free;
    }
    ((free.max(0) + order_coins) * UNSAID_SHARE_PCT / 100 - order_coins).max(0)
}
/// Seconds after the daemon start before unwalked bids are adopted: the
/// owners' passes (`keep_bid`: gear, stock, tools) re-register theirs
/// first, with their live ceiling.
pub const ADOPT_GRACE_SECS: f64 = 600.0;

fn daemon_start() -> f64 {
    static START: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *START.get_or_init(wall_now)
}

/// Resting bids nobody walks (user 2026-11-20, « il faut les faire
/// monter les offres d'achat pour inciter les gens à vendre »: live,
/// bids at 1 coin frozen 18 to 67 h, their walk lost at a daemon
/// restart): `(order, ceiling)`, the stored ceiling or, for an orphan
/// bid (no motive), `UNSAID_CEILING`.
/// The market makers' quotes climb in their own pass (`maker::quote`).
pub fn bid_adoptions(rows: &[lifescape_store::Order], handled: &dyn Fn(i64) -> bool) -> Vec<(i64, i64)> {
    rows.iter()
        .filter(|row| row.side == lifescape_store::OrderSide::Buy && resting(row) && row.motive != crate::market_lineage::MARKET_MAKE)
        .filter(|row| !handled(row.order_id))
        // A bid of a live motive (need, gear, stock, tool...) without a
        // stored ceiling waits for its owner's pass (`keep_bid`: its real
        // ceiling); only an orphan (no motive) walks to `UNSAID_CEILING`.
        .filter(|row| row.ceiling.is_some() || row.motive.is_empty())
        .map(|row| (row.order_id, row.ceiling.unwrap_or(UNSAID_CEILING).max(row.price)))
        .collect()
}

/// Adopts every bid nobody walks into the 10 % walk (`bid_adoptions`),
/// once the grace after the daemon start ran. Returns the count.
pub fn adopt_bids(service: &MarketService, wall: f64) -> usize {
    if wall - daemon_start() < ADOPT_GRACE_SECS {
        return 0;
    }
    let rows = service.lock_store().resting_orders().unwrap_or_default();
    let mut adopted = 0;
    for (order, ceiling) in bid_adoptions(&rows, &|order| handled(service, order)) {
        let Some(row) = rows.iter().find(|row| row.order_id == order) else { continue };
        if adopt(service, order, QuoteBounds::for_row(row, row.price, ceiling)) {
            adopted += 1;
        }
    }
    if adopted > 0 {
        crate::logln!("market-reprice: {adopted} resting bid(s) adopted by the 10 % walk");
    }
    adopted
}

/// Re-adopts a resting order the engine has no bounds for (daemon
/// restart, listings placed before demand pricing): it walks from now.
/// Returns false when the order already walks.
/// Idempotent (2026-11-14): an order id adopted once is never adopted
/// again, even if its bounds were dropped since.
pub fn adopt(service: &MarketService, order: i64, bounds: QuoteBounds) -> bool {
    let mut engine = service.lock_engine();
    if engine.quote_bounds.contains_key(&order) || !engine.adopted.insert(order) {
        return false;
    }
    engine.quote_bounds.insert(order, bounds);
    true
}

/// True once `order` walks or was adopted (the selling pass skips it
/// before any reserve or book work).
pub fn handled(service: &MarketService, order: i64) -> bool {
    let engine = service.lock_engine();
    engine.quote_bounds.contains_key(&order) || engine.adopted.contains(&order)
}

/// Outcome of `keep_bid`.
#[derive(Debug, Clone, PartialEq)]
pub enum StandingBid {
    /// A new bid was placed at this price.
    Placed { order: i64, price: i64 },
    /// The resting bid keeps its walk; its ceiling follows the wallet.
    Kept { order: i64, price: i64 },
}

/// A standing bid (D-B: it waits for a producer) of `account` for `qty`
/// of `item` with the per-unit cap `ceiling`. A resting bid keeps its
/// price and walk (re-placing it every pass reset the walk to the
/// reference: live 2026-11-13, wishes never climbed past a few coins);
/// only its ceiling follows the wallet, and a price over a shrunk
/// ceiling is amended down to it. Otherwise a bid opens at the covered
/// ask, else at the bot's own last fill on the item, else the reference
/// (`remembered_bid`, 2026-11-14).
pub fn keep_bid(service: &MarketService, account: &str, item: i64, qty: i64, ceiling: i64, motive: &str) -> Result<StandingBid, String> {
    keep_bid_from(service, account, item, qty, 1, ceiling, motive)
}

/// `keep_bid` whose NEW bid opens at least at `open_at` (never over the
/// ceiling): what the buyer knows the object is worth (the gear plan's
/// expected price, 2026-11-14: a never-traded platebody bid opened at 1
/// and climbed 5 % per window for hours). A resting bid keeps its walk,
/// raised first to `open_at` when under it (2026-11-18).
/// Lineage (plan marché M2, C3): the resting bid of the same `motive` is
/// taken first, else any resting bid of the account on the object (one
/// bid per object, never a second beside it); its lineage, t0 and price
/// go on, never a cancel and a lower repose.
pub fn keep_bid_from(service: &MarketService, account: &str, item: i64, qty: i64, open_at: i64, ceiling: i64, motive: &str) -> Result<StandingBid, String> {
    // Plan prix lot D: the caller prices per unit; the walk below runs in
    // the book's unit (per lot) and answers per unit.
    let lot = crate::market_units::lot_of(item);
    let qty = lifescape_market::lots::whole_lots_up(qty, lot);
    let ceiling = lifescape_market::lots::lot_price(ceiling.max(1), lot);
    let open_at = lifescape_market::lots::lot_price(open_at, lot);
    let answer = |bid: StandingBid| match bid {
        StandingBid::Placed { order, price } => StandingBid::Placed { order, price: lifescape_market::lots::unit_price(price, lot) },
        StandingBid::Kept { order, price } => StandingBid::Kept { order, price: lifescape_market::lots::unit_price(price, lot) },
    };
    // Plan prix §2 (user 2026-11-18): a bid opens under its ceiling by the
    // buyer's margin (a course to concede); a blocking need bids what the
    // object is worth to it.
    let open_cap = if motive == crate::market_lineage::NEED { ceiling } else { lifescape_market::discovery::bid_under_ceiling(ceiling, crate::market_discovery::account_margin(service, account)) };
    let rows: Vec<lifescape_store::Order> = service
        .lock_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.item_id == item && row.side == lifescape_store::OrderSide::Buy && resting(row))
        .collect();
    let open = rows.iter().find(|row| row.motive == motive).or_else(|| rows.first()).cloned();
    if let Some(row) = open {
        let (order, price) = amend_resting(service, account, &row, resting_target(row.price, open_at, open_cap, ceiling), None, ceiling)?;
        return Ok(answer(StandingBid::Kept { order, price }));
    }
    let reference = service.reference_lot(item);
    let (_, ask) = service.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
    let remembered = lifescape_market::remembered_bid(reference.price, ask, ceiling, service.own_buy_price_lot(account, item));
    // A covered ask is taken at its price; otherwise the bid opens at
    // what the buyer knows the object is worth, at least. Never traded
    // (M7): on the public anchors, under that worth (`open_at`, its use
    // value when it knows one), over the sellers' public floor.
    let covered = ask.is_some_and(|ask| ask >= 1 && ask <= ceiling);
    // Discovery prices per unit (public anchors): into the book's unit.
    let discovered = if covered || reference.traded {
        None
    } else {
        discovery_bid(service, account, item, (open_at > lot).then_some(lifescape_market::lots::unit_price(open_at, lot)), lifescape_market::lots::unit_price(ceiling, lot)).map(|unit| lifescape_market::lots::lot_price(unit, lot))
    };
    let price = match discovered {
        _ if covered => remembered,
        Some(open) => open.max(open_at.min(open_cap)).min(open_cap),
        None => remembered.max(open_at.min(open_cap)).min(open_cap),
    };
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value("buy")),
        ("item", Value::Int(item)),
        ("qty", Value::Int(qty.max(1))),
        ("price", Value::Int(price)),
        ("ceiling", Value::Int(ceiling.max(price))),
        ("motive", str_value(motive)),
        ("per_lot", Value::Bool(true)),
    ]));
    if reply.get("ok").as_bool() != Some(true) {
        return Err(reply.get("error").as_str().unwrap_or("place refused").to_string());
    }
    Ok(answer(StandingBid::Placed { order: reply.get("order").as_i64().unwrap_or(-1), price }))
}

/// Price a resting bid at `price` moves to (book unit), `None` to stay:
/// over the ceiling, down to it; under what the buyer knows the object
/// is worth (`open_at`, user 2026-11-18 levier 2), up to that worth at
/// once (never past `open_cap`), never a slow walk from a noise price
/// while producers read no demand.
fn resting_target(price: i64, open_at: i64, open_cap: i64, ceiling: i64) -> Option<i64> {
    if price > ceiling {
        Some(ceiling)
    } else {
        Some(open_at.min(open_cap)).filter(|worth| *worth > price)
    }
}

/// Amends the resting bid `row` to `target` (book unit) and its remainder
/// down to `max_remaining` units when it holds more (never up), in ONE
/// amend (its lineage goes on); then its walk's ceiling becomes
/// `ceiling`. Returns its (order, price) in the book's unit.
fn amend_resting(service: &MarketService, account: &str, row: &lifescape_store::Order, target: Option<i64>, max_remaining: Option<i64>, ceiling: i64) -> Result<(i64, i64), String> {
    let mut order = row.order_id;
    let mut price = row.price;
    let trim = max_remaining.filter(|cap| *cap >= 1 && *cap < row.remaining);
    if target.is_some() || trim.is_some() {
        let next = target.unwrap_or(row.price);
        let mut request = vec![("op", str_value("amend")), ("account", str_value(account)), ("order", Value::Int(order)), ("price", Value::Int(next)), ("per_lot", Value::Bool(true))];
        if let Some(cap) = trim {
            request.push(("max_remaining", Value::Int(cap)));
        }
        let reply = service.handle(&object(request));
        if reply.get("ok").as_bool() != Some(true) {
            return Err(reply.get("error").as_str().unwrap_or("amend refused").to_string());
        }
        service.lock_engine().quote_bounds.remove(&order);
        order = reply.get("order").as_i64().unwrap_or(order);
        price = next;
    }
    // The persisted ceiling follows the owner's limit (B1): a restart
    // adopts the bid with it, never with a stale wallet-sized one.
    if order != row.order_id || row.ceiling != Some(ceiling) {
        if let Err(error) = service.lock_store().set_order_ceiling(order, ceiling) {
            crate::logln!("market-reprice: ceiling of order {order} not kept: {error}");
        }
    }
    let mut engine = service.lock_engine();
    let bounds = engine.quote_bounds.entry(order).or_insert_with(|| QuoteBounds::for_row(row, price, ceiling));
    bounds.floor = bounds.floor.min(ceiling);
    bounds.ceiling = ceiling;
    // A cut is no fill (re-review lot 4): the walk's last remainder drops
    // by the units cut, so the next window never reads them as filled
    // (backoff, false own fill); a fill seen before the cut still shows.
    if let Some(cap) = trim {
        bounds.last_remaining = (bounds.last_remaining - (row.remaining - cap)).max(0);
    }
    Ok((order, price))
}

/// What `refresh_resting_bid` did to a resting bid: prices per unit, its
/// walk's ceiling before (`None`: the engine had no bounds, a restart)
/// and after, its remainder before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct RestingRefresh {
    pub order: i64,
    pub price: i64,
    pub old_price: i64,
    pub ceiling: i64,
    pub old_ceiling: Option<i64>,
    pub remaining: i64,
    pub old_remaining: i64,
}

impl RestingRefresh {
    /// True when the price, the ceiling or the remainder moved.
    pub fn changed(&self) -> bool {
        self.price != self.old_price || self.old_ceiling != Some(self.ceiling) || self.remaining != self.old_remaining
    }
}

/// The resting bid of `account` on `item` with `motive` brought to the
/// bounds of now (plan équilibre step 4, review lot 4): its price as
/// `keep_bid_from` moves a kept bid (up to `open_at` when under it, down
/// to `ceiling` when over it), its remainder cut to `quantity` (the need
/// of now, rounded up to whole lots; never raised), its walk's ceiling
/// replaced; lineage and walk go on. `Ok(None)` when no such bid rests
/// (filled or gone meanwhile): the caller never places one from here.
pub fn refresh_resting_bid(service: &MarketService, account: &str, item: i64, quantity: i64, open_at: i64, ceiling: i64, motive: &str) -> Result<Option<RestingRefresh>, String> {
    let lot = crate::market_units::lot_of(item);
    let quantity = lifescape_market::lots::whole_lots_up(quantity.max(1), lot);
    let ceiling = lifescape_market::lots::lot_price(ceiling.max(1), lot);
    let open_at = lifescape_market::lots::lot_price(open_at, lot);
    let open_cap = if motive == crate::market_lineage::NEED { ceiling } else { lifescape_market::discovery::bid_under_ceiling(ceiling, crate::market_discovery::account_margin(service, account)) };
    let row = service
        .lock_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .find(|row| row.item_id == item && row.side == lifescape_store::OrderSide::Buy && resting(row) && row.motive == motive);
    let Some(row) = row else { return Ok(None) };
    let old_ceiling = service.lock_engine().quote_bounds.get(&row.order_id).map(|bounds| bounds.ceiling);
    let (order, price) = amend_resting(service, account, &row, resting_target(row.price, open_at, open_cap, ceiling), Some(quantity), ceiling)?;
    let remaining = service.lock_store().get_order(order).ok().flatten().map(|stored| stored.remaining).unwrap_or(row.remaining);
    let unit = |price: i64| lifescape_market::lots::unit_price(price, lot);
    Ok(Some(RestingRefresh { order, price: unit(price), old_price: unit(row.price), ceiling: unit(ceiling), old_ceiling: old_ceiling.map(unit), remaining, old_remaining: row.remaining }))
}

/// Opening of a bid on the never-traded book `item` (M7,
/// `market_discovery::buyer_open`); `None` without anchors.
pub(crate) fn discovery_bid(service: &MarketService, account: &str, item: i64, value: Option<i64>, ceiling: i64) -> Option<i64> {
    let name = usize::try_from(crate::market_audit::base_of(item)).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name)?;
    crate::market_discovery::buyer_open(service, account, name, value, ceiling)
}

/// Registered order count (dashboard and tests).
pub fn tracked(service: &MarketService) -> usize {
    service.lock_engine().quote_bounds.len()
}

#[cfg(test)]
#[path = "market_reprice_tests.rs"]
mod tests;
