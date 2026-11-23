#![forbid(unsafe_code)]
//! Seller side of the economy (2026-11-14, user rules 1, 4, 5, 6 and the
//! anti-dumping addition): every listing is priced from the seller's OWN
//! cost (`brain_reserve`), opens high when nobody traded the object, is
//! sized to the public demand, and never walks under that reserve. A bot
//! remembers its own fills. Every input is the bot's own data plus
//! public market data (book, prints); never another bot's private state.
//! (The merchant flip quotes were removed in wave 3: dedicated market
//! makers quote their slice, `market_maker`.)
//!
//! Shared by the brain's default selling (`brain_sales`) and the worker
//! script (`market sell`) through the `seller_quote` op.

use lifescape_json::{object, str_value, Value};
use lifescape_market::{discovery, found_open, OwnFill, Reference};
use lifescape_planner::market_fit::{sale_lot, BookView};
use lifescape_planner::Life;

use crate::brain_reserve::{CostSource, OwnCosts, Reserve};
use crate::market_service::MarketService;

/// Outcome of one sale decision.
#[derive(Clone, Debug, PartialEq)]
pub enum SaleDecision {
    /// List `qty` at `price`, walking down to `floor` (the reserve).
    List { qty: i64, price: i64, floor: i64 },
    /// Keep the stock banked (no own cost known, nothing free, or the
    /// reserve over the anti-bug band of a traded reference).
    Hold(String),
}

/// What a lot prices against: for FOUND stock (`found_open`) the
/// cheapest ask of ANOTHER account (the seller's own listing never
/// anchors its own opening) and the object's high alchemy value; for
/// every lot the seller's outside option and its urgency (wave 2).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FoundMarks {
    pub others_ask: Option<i64>,
    pub anchor: Option<i64>,
    /// A full bank (D17, M4/M5): the lot hits a bid at or over its
    /// floor, or undercuts the cheapest ask by one relative step.
    pub pressed: bool,
    /// Public discovery anchors (M7, `market_discovery::seller_anchors`):
    /// the floor never under the shop's offer nor the seller's own cast,
    /// a never-traded object opening over the published cost.
    pub discovery: lifescape_market::Anchors,
    /// The seller's profile margin (`discovery::margin_of`).
    pub margin: f64,
    /// Plan prix §1 (user 2026-11-18): horizons this seller's listings of
    /// the object ended unsold over the last day, net of its sales
    /// (`discovery::unsold_horizons`): kept for the report, no longer lowers the walk limit (2026-11-22).
    pub unsold: i64,
}

/// One sale (pure): the opening ask from the seller's reserve and own
/// last fill (`seller_open`), a lot sized to the public demand
/// (`market_fit::sale_lot`: bids + a day of volume, at least one
/// inventory), never under the reserve (no reference band, user rule
/// 2026-11-15). Found
/// stock (`CostSource::Found`: loot, drops, ground spawns) opens at the
/// clearing price instead (`found_open`: current price, just under the
/// cheapest ask of another account for an untraded object, else its
/// high alchemy value, `FoundMarks`) and walks down to 1. M7: the floor
/// is never under the public outside options (`discovery::seller_floor`)
/// and a never-traded object opens on its public anchors
/// (`discovery::listing_open`, `discovery::loot_anchor`: over the
/// published production cost, under the shop's selling price).
pub fn plan_sale(free: i64, reserve: Option<Reserve>, reference: Reference, best_bid: Option<i64>, last_sell: Option<OwnFill>, marks: FoundMarks, view: &BookView) -> SaleDecision {
    let Some(reserve) = reserve else { return SaleDecision::Hold("no own cost known".to_string()) };
    let qty = sale_lot(free, Some(view));
    if qty <= 0 {
        return SaleDecision::Hold("nothing free".to_string());
    }
    // The walk limit: the seller's OWN value of the unit (its cost, 1 coin
    // for found stock), never lowered by unsold horizons (user
    // 2026-11-22, the mirror of B1); no shop, alchemy or overstock floor
    // (user 2026-11-18: "retire les floor de prix").
    let keep = reserve.coins.max(1);
    let (price, floor) = if reserve.source == CostSource::Found {
        let anchor = discovery::loot_anchor(reference, marks.anchor, keep, &marks.discovery, marks.margin);
        found_open(reference, best_bid, marks.others_ask, keep, anchor, last_sell, marks.pressed)
    } else {
        match best_bid.filter(|bid| marks.pressed && *bid >= keep) {
            // M4: a full bank hits a bid at or over its floor.
            Some(bid) => return SaleDecision::List { qty, price: bid, floor: keep },
            None => discovery::listing_open(reference, best_bid, keep, last_sell, &marks.discovery, marks.margin),
        }
    };
    // Plan prix §2: never opened on the floor (the concession needs a
    // course), unless a pressed seller hits a bid at its floor.
    let price = if marks.pressed && best_bid.is_some_and(|bid| bid == price) { price } else { price.max(discovery::open_over_floor(floor, marks.margin)) };
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
    /// Over today's opening (an ask anchored on an old reserve or an
    /// outlier, frank_lane's staff at 1644 against asks at 17): amended
    /// down to it, then walks.
    Lower { price: i64, floor: i64 },
    /// Withdrawn (stock back to the bank): over the demand lot, or no own
    /// cost known. The next selling pass lists a lot.
    Withdraw(String),
}

/// Adoption of one resting listing (pure); `opening` = the ask the
/// seller would post for it today (`plan_sale`).
pub fn adoption(price: i64, remaining: i64, reserve: Option<i64>, lot: i64, opening: Option<i64>) -> Adoption {
    let Some(reserve) = reserve else { return Adoption::Withdraw("no own cost known".to_string()) };
    if remaining > lot {
        return Adoption::Withdraw(format!("{remaining} over the demand lot {lot}"));
    }
    if price < reserve {
        return Adoption::Raise { price: reserve, floor: reserve };
    }
    match opening.filter(|opening| *opening >= reserve && *opening < price) {
        Some(opening) => Adoption::Lower { price: opening, floor: reserve },
        None => Adoption::Walk { floor: reserve },
    }
}

/// A resting listing that offers a sliver of a big free stack (pure,
/// 2026-11-14, frank_lane: 1 iron dagger listed, 40 banked, the stack
/// never listed because the item had a listing): untouched (`remaining
/// == quantity`), more units free beside it, and at most half the lot
/// the whole stack would list (`lot_of_whole` = `sale_lot(remaining +
/// free)`). It is withdrawn and the stack lists as one lot.
pub fn undersized(quantity: i64, remaining: i64, free: i64, lot_of_whole: i64) -> bool {
    remaining == quantity && free > 0 && quantity.saturating_mul(2) <= lot_of_whole
}

/// The `bid_response` make cost of `name` for `account` (its life and
/// live snapshot from the supervisor), the reserve's cap; `None` when
/// the bot is not supervised or cannot make it.
fn make_cost_of(life: Option<&Life>, account: &str, name: &str, units: i64) -> Option<f64> {
    let life = life?;
    let live = crate::jobs::live_of(account)?;
    crate::brain_bids::production_costs_for(life, &live, &[(name.to_string(), units.max(1))]).remove(name)
}

/// The seller's pricing policy on the engine (the engine itself keeps
/// only the memory: `lifescape_orch_market::market_own`).
pub trait SellerPolicy {
    /// The account's sale decision for `free` units of `name` (`item`),
    /// its reserve capped at `make_cost` (`brain_bids::production_costs_for`)
    /// and learned down per unsold horizon to 1 coin (no shop, alchemy or
    /// overstock floor, user 2026-11-18), `pressed` when its bank is full
    /// (M4/M5); a never-traded object opens on its public anchors (M7,
    /// `market_discovery`, the own cast's net `own_alch` among them).
    #[allow(clippy::too_many_arguments)]
    fn sale_decision(&self, account: &str, name: &str, item: i64, free: i64, unsold: i64, costs: &OwnCosts, life: Option<&Life>, make_cost: Option<f64>, pressed: bool, own_alch: Option<i64>) -> (SaleDecision, Option<Reserve>);
}

impl SellerPolicy for MarketService {
    /// The account's sale decision for `free` units of `name` (`item`),
    /// its reserve capped at `make_cost` (`brain_bids::production_costs_for`)
    /// and learned down per unsold horizon to 1 coin (no shop, alchemy or
    /// overstock floor, user 2026-11-18), `pressed` when its bank is full
    /// (M4/M5); a never-traded object opens on its public anchors (M7,
    /// `market_discovery`, the own cast's net `own_alch` among them).
    #[allow(clippy::too_many_arguments)]
    fn sale_decision(&self, account: &str, name: &str, item: i64, free: i64, unsold: i64, costs: &OwnCosts, life: Option<&Life>, make_cost: Option<f64>, pressed: bool, own_alch: Option<i64>) -> (SaleDecision, Option<Reserve>) {
        let reserve = {
            let store = self.lock_store();
            costs.reserve(&store, name, item, life, make_cost)
        };
        let reference = self.reference(item);
        let (bid, _) = self.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        let bid = bid.map(|price| crate::market_units::to_unit(item, price));
        let last = self.own_sell_fill(account, item);
        let others_ask = self.lock_engine().books.get(&item).and_then(|book| {
            book.orders().into_iter().filter(|order| order.side == lifescape_market::Side::Sell && order.remaining > 0 && order.account != account && order.price >= 1).map(|order| order.price).min()
        });
        let others_ask = others_ask.map(|price| crate::market_units::to_unit(item, price));
        let marks = FoundMarks {
            others_ask,
            anchor: lifescape_planner::alchemy::alch_value(name, true),
            pressed,
            discovery: crate::market_discovery::seller_anchors(name, own_alch),
            margin: crate::market_discovery::account_margin(self, account),
            unsold,
        };
        (plan_sale(free, reserve, reference, bid, last, marks, &self.public_view(account, item)), reserve)
    }
}

/// `seller_quote {account, item, qty}`: the account's own listing for
/// `qty` free units (the `market sell` script's default price).
pub fn seller_quote(service: &MarketService, request: &Value) -> Value {
    let account = request.get("account").as_str().unwrap_or("").to_string();
    // One book per object: a banknote is quoted as its object.
    let item = request.get("item").as_i64().map(crate::market_audit::book_item).unwrap_or(-1);
    let qty = request.get("qty").as_i64().unwrap_or(0);
    let Some(name) = usize::try_from(item).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name) else {
        return MarketService::err(format!("unknown item {item}"));
    };
    // The time value first (D1): its refresh locks the store.
    let rate = crate::brain_d17::coin_rate(&account);
    let costs = OwnCosts::load(&service.lock_store(), &account, rate);
    let life = crate::brain::load_life(&account);
    let make_cost = make_cost_of(life.as_ref(), &account, name, qty);
    let pressed = request.get("urgent").as_bool() == Some(true);
    let live = crate::jobs::live_of(&account);
    let own_alch = live.as_ref().and_then(|live| crate::brain_alch::alch_floor(live, name));
    let unsold = service.unsold_horizons(&account, item);
    let (decision, reserve) = service.sale_decision(&account, name, item, qty, unsold, &costs, life.as_ref(), make_cost, pressed, own_alch);
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

#[cfg(test)]
#[path = "market_seller_tests.rs"]
mod tests;
