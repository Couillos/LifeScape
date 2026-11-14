#![forbid(unsafe_code)]
//! Seller side of the economy (2026-11-14, user rules 1, 4, 5, 6 and the
//! anti-dumping addition): every listing is priced from the seller's OWN
//! cost (`brain_reserve`), opens high when nobody traded the object, is
//! sized to the public demand, and never walks under that reserve. A bot
//! remembers its own fills. Merchants price flips off the book with a
//! margin that pays their time. Every input is the bot's own data plus
//! public market data (book, prints); never another bot's private state.
//!
//! Shared by the brain's default selling (`brain_sales`) and the worker
//! scripts (`market sell`, `market flip`) through the `seller_quote` and
//! `flip_quote` ops.

use lifescape_json::{object, str_value, Value};
use lifescape_market::{seller_open, OwnFill, Reference};
use lifescape_planner::market_fit::{sale_lot, BookView};
use lifescape_planner::Life;

use crate::brain_reserve::{OwnCosts, Reserve};
use crate::market_service::MarketService;

/// Planner estimate of one flip unit when the merchant never measured
/// one: the `market:flip` engagement (`est_ticks` 6000,
/// `resolve_obtain::resolve_money`) over the 100 units a flip carries
/// (`brain_launch`).
pub const FLIP_EST_TICKS_PER_UNIT: f64 = 6000.0 / 100.0;

/// Outcome of one sale decision.
#[derive(Clone, Debug, PartialEq)]
pub enum SaleDecision {
    /// List `qty` at `price`, walking down to `floor` (the reserve).
    List { qty: i64, price: i64, floor: i64 },
    /// Keep the stock banked (no own cost known, nothing free, or the
    /// reserve over the anti-bug band of a traded reference).
    Hold(String),
}

/// One sale (pure): the opening ask from the seller's reserve and own
/// last fill (`seller_open`), a lot sized to the public demand
/// (`market_fit::sale_lot`: bids + a day of volume, at least one
/// inventory), the 5x band of a traded reference respected (the opening
/// comes down to it, never under the reserve: over it, hold).
pub fn plan_sale(free: i64, reserve: Option<Reserve>, reference: Reference, best_bid: Option<i64>, last_sell: Option<OwnFill>, view: &BookView) -> SaleDecision {
    let Some(reserve) = reserve else { return SaleDecision::Hold("no own cost known".to_string()) };
    let qty = sale_lot(free, Some(view));
    if qty <= 0 {
        return SaleDecision::Hold("nothing free".to_string());
    }
    let (mut price, floor) = seller_open(reference, best_bid, reserve.coins, last_sell);
    if reference.traded {
        let band = reference.price.saturating_mul(5);
        if floor > band {
            return SaleDecision::Hold(format!("reserve {floor} over the 5x band of reference {}", reference.price));
        }
        price = price.min(band);
    }
    SaleDecision::List { qty, price, floor }
}

/// What to do with a resting listing that walks no more (placed before
/// a daemon restart or before these rules).
#[derive(Clone, Debug, PartialEq)]
pub enum Adoption {
    /// Walk with demand down to the reserve.
    Walk { floor: i64 },
    /// Under the reserve: amended up to it, then walks.
    Raise { price: i64, floor: i64 },
    /// Withdrawn (stock back to the bank): over the demand lot, or no own
    /// cost known. The next selling pass lists a lot.
    Withdraw(String),
}

/// Adoption of one resting listing (pure).
pub fn adoption(price: i64, remaining: i64, reserve: Option<i64>, lot: i64) -> Adoption {
    let Some(reserve) = reserve else { return Adoption::Withdraw("no own cost known".to_string()) };
    if remaining > lot {
        return Adoption::Withdraw(format!("{remaining} over the demand lot {lot}"));
    }
    if price < reserve {
        return Adoption::Raise { price: reserve, floor: reserve };
    }
    Adoption::Walk { floor: reserve }
}

/// Minimum flip margin per unit (pure): the merchant's ticks per flipped
/// unit (measured from its own market reports, else the planner
/// estimate) at its coins per tick, at least 1 coin.
pub fn flip_margin(ticks_per_unit: Option<f64>, coins_per_tick: f64) -> i64 {
    crate::brain_reserve::reserve_coins(ticks_per_unit.unwrap_or(FLIP_EST_TICKS_PER_UNIT), coins_per_tick)
}

/// Measured ticks per flipped unit: the merchant's own `market` reports,
/// ticks over the units it traded (bought or sold, coins aside).
pub fn flip_ticks_per_unit(reports: &[crate::brain_reserve::OwnReport]) -> Option<f64> {
    let mut ticks = 0i64;
    let mut units = 0i64;
    for report in reports.iter().filter(|report| report.method == "market" && report.ticks > 0) {
        let traded: i64 = report
            .lines
            .iter()
            .filter(|line| line.provenance == "traded" && !crate::brain_reserve::is_coins(&line.item))
            .map(|line| line.quantity.abs())
            .sum();
        if traded > 0 {
            ticks += report.ticks;
            units += traded;
        }
    }
    (units > 0).then(|| ticks as f64 / units as f64)
}

/// The `bid_response` make cost of `name` for `account` (its life and
/// live snapshot from the supervisor), the reserve's cap; `None` when
/// the bot is not supervised or cannot make it.
fn make_cost_of(life: Option<&Life>, account: &str, name: &str, units: i64) -> Option<f64> {
    let life = life?;
    let live = crate::supervise::live_of(account)?;
    crate::brain_bids::production_costs_for(life, &live, &[(name.to_string(), units.max(1))]).remove(name)
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

impl MarketService {
    /// The public book of an item as a seller reads it (`BookView`):
    /// every OTHER account's bids, every resting ask, 24 h volume.
    pub(crate) fn public_view(&self, account: &str, item: i64) -> BookView {
        let clock = Self::now_clock();
        // One indexed SUM, never the rows (CPU: hottest pool call).
        let volume_24h: i64 = self.lock_store().volume_between(item, clock - 86_400, clock + 1).unwrap_or(0);
        let mut view = BookView { volume_24h, ..BookView::default() };
        if let Some(book) = self.lock_engine().books.get(&item) {
            for order in book.orders().into_iter().filter(|order| order.remaining > 0) {
                if order.side == lifescape_market::Side::Sell {
                    view.asks.push((order.price, order.remaining));
                } else if order.account != account {
                    view.bids.push((order.price, order.remaining));
                }
            }
        }
        view
    }

    /// Records the seller's own fill with how long its listing rested
    /// (from `QuoteBounds::listed_wall`; a sell that crossed a bid on
    /// arrival rested 0). Called by every match (`record_fill`).
    pub(crate) fn remember_fill(&self, fill: &lifescape_market::Fill) {
        let tick_ms = crate::market_eta::live_tick_ms().max(1) as f64;
        let mut engine = self.lock_engine();
        let listed = engine.quote_bounds.get(&(fill.sell_id as i64)).map(|bounds| bounds.listed_wall);
        let rested_ticks = match listed {
            Some(wall) => Some(((wall_now() - wall).max(0.0) * 1000.0 / tick_ms) as u64),
            None if fill.taker == lifescape_market::Side::Sell => Some(0),
            None => None,
        };
        engine.own_sells.insert((fill.seller.clone(), fill.item), OwnFill { price: fill.price, rested_ticks });
    }

    /// The account's own last sale of `item`: memory (with its speed),
    /// else its newest non-failed seller deal (speed unknown).
    pub(crate) fn own_sell_fill(&self, account: &str, item: i64) -> Option<OwnFill> {
        if let Some(fill) = self.lock_engine().own_sells.get(&(account.to_string(), item)).copied() {
            return Some(fill);
        }
        let stored = self.lock_store().last_own_fill(account, item, false).ok().flatten();
        stored.map(|(price, _)| OwnFill { price, rested_ticks: None })
    }

    /// The account's own last purchase price of `item` (non-failed deal).
    pub(crate) fn own_buy_price(&self, account: &str, item: i64) -> Option<i64> {
        self.lock_store().last_own_fill(account, item, true).ok().flatten().map(|(price, _)| price)
    }

    /// The account's sale decision for `free` units of `name` (`item`),
    /// its reserve capped at `make_cost` (`brain_bids::production_costs_for`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sale_decision(&self, account: &str, name: &str, item: i64, free: i64, costs: &OwnCosts, life: Option<&Life>, make_cost: Option<f64>) -> (SaleDecision, Option<Reserve>) {
        let reserve = {
            let store = self.lock_store();
            costs.reserve(&store, name, item, life, make_cost)
        };
        let reference = self.reference(item);
        let (bid, _) = self.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        let last = self.own_sell_fill(account, item);
        (plan_sale(free, reserve, reference, bid, last, &self.public_view(account, item)), reserve)
    }

    /// `seller_quote {account, item, qty}`: the account's own listing for
    /// `qty` free units (the `market sell` script's default price).
    pub(crate) fn op_seller_quote(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let item = request.get("item").as_i64().unwrap_or(-1);
        let qty = request.get("qty").as_i64().unwrap_or(0);
        let Some(name) = usize::try_from(item).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name) else {
            return Self::err(format!("unknown item {item}"));
        };
        let costs = OwnCosts::load(&self.lock_store(), &account);
        let life = crate::brain::load_life(&account);
        let make_cost = make_cost_of(life.as_ref(), &account, name, qty);
        let (decision, reserve) = self.sale_decision(&account, name, item, qty, &costs, life.as_ref(), make_cost);
        let reserve_value = reserve.map(|known| Value::Int(known.coins)).unwrap_or(Value::Null);
        match decision {
            SaleDecision::List { qty, price, floor } => object(vec![
                ("ok", Value::Bool(true)),
                ("qty", Value::Int(qty)),
                ("price", Value::Int(price)),
                ("floor", Value::Int(floor)),
                ("reserve", reserve_value),
            ]),
            SaleDecision::Hold(why) => object(vec![("ok", Value::Bool(true)), ("hold", str_value(&why)), ("reserve", reserve_value)]),
        }
    }

    /// `flip_quote {account, item, qty}`: a merchant's legs off the book
    /// (`lifescape_market::flip_quotes`) with its own minimum margin; the
    /// sell leg never under what the stock cost it plus that margin, its
    /// lot sized to the public demand. `sell` is null (hold) when the
    /// stock has no known own cost or its floor is over the 5x band.
    pub(crate) fn op_flip_quote(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("").to_string();
        let item = request.get("item").as_i64().unwrap_or(-1);
        let held = request.get("qty").as_i64().unwrap_or(0);
        let Some(name) = usize::try_from(item).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name) else {
            return Self::err(format!("unknown item {item}"));
        };
        let costs = OwnCosts::load(&self.lock_store(), &account);
        let margin = flip_margin(flip_ticks_per_unit(&costs.reports), costs.coins_per_tick);
        let sell_floor = match self.own_buy_price(&account, item) {
            Some(paid) => Some(paid + margin),
            None => {
                let life = crate::brain::load_life(&account);
                let make_cost = make_cost_of(life.as_ref(), &account, name, held);
                let store = self.lock_store();
                costs.reserve(&store, name, item, life.as_ref(), make_cost).map(|reserve| reserve.coins)
            }
        };
        let (bid, ask) = self.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        let quote = lifescape_market::flip_quotes(bid, ask, margin, sell_floor.unwrap_or(1));
        let reference = self.reference(item);
        let band = reference.traded.then(|| reference.price.saturating_mul(5));
        let sell = sell_floor.filter(|floor| band.is_none_or(|cap| *floor <= cap)).map(|_| band.map_or(quote.sell, |cap| quote.sell.min(cap)));
        let lot = sale_lot(held, Some(&self.public_view(&account, item)));
        object(vec![
            ("ok", Value::Bool(true)),
            ("buy", quote.buy.map(Value::Int).unwrap_or(Value::Null)),
            ("sell", sell.map(Value::Int).unwrap_or(Value::Null)),
            ("sell_floor", sell_floor.map(Value::Int).unwrap_or(Value::Null)),
            ("lot", Value::Int(lot)),
            ("margin", Value::Int(margin)),
        ])
    }
}

#[cfg(test)]
#[path = "market_seller_tests.rs"]
mod tests;
