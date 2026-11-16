#![forbid(unsafe_code)]
//! ZIP-style quoting plus merchant risk caps. A quoter starts from the
//! reference with its margin, walks toward the other side while waiting,
//! and never crosses its floor (reserve value) or ceiling (shop price or
//! time-saved value). Merchants pick wide, busy books and size inside
//! fixed shares of capital, stock and book depth.

use crate::error::Error;

/// One side's quoting state. Margins are fractions (0.10 = 10%).
#[derive(Clone, Debug, PartialEq)]
pub struct ZipQuote {
    /// Starting offset from the reference: + for sells, - for buys.
    pub margin: f64,
    /// Ticks of no fill before stepping one coin toward the market.
    pub patience_ticks: u64,
    /// Reserve value: never quote a sell below this.
    pub floor: i64,
    /// Most a buyer may pay: shop price or time-saved value.
    pub ceiling: i64,
}

impl ZipQuote {
    /// Opening quote from the reference. Sell when `sell` is true.
    pub fn opening(&self, reference: i64, sell: bool) -> Result<i64, Error> {
        if self.floor < 1 || self.ceiling < 1 || self.floor > self.ceiling {
            return Err(Error::Invalid(format!(
                "bad zip bounds {}/{}",
                self.floor, self.ceiling
            )));
        }
        if !(0.0..=1.0).contains(&self.margin) {
            return Err(Error::Invalid(format!("bad zip margin {}", self.margin)));
        }
        let raw = if sell {
            reference as f64 * (1.0 + self.margin)
        } else {
            reference as f64 * (1.0 - self.margin)
        };
        Ok(clamp_quote(raw.round() as i64, self.floor, self.ceiling))
    }

    /// Next quote after waiting: one coin toward the market per patience
    /// window, still inside the bounds.
    pub fn stepped(&self, current: i64, waited_ticks: u64, sell: bool) -> i64 {
        if self.patience_ticks == 0 || waited_ticks < self.patience_ticks {
            return clamp_quote(current, self.floor, self.ceiling);
        }
        let steps = (waited_ticks / self.patience_ticks.max(1)).min(1_000_000) as i64;
        let moved = if sell { current - steps } else { current + steps };
        clamp_quote(moved, self.floor, self.ceiling)
    }
}

/// One repricing step toward the other side (PRD3 lot 2): 5% of the
/// current price, at least one coin, kept inside `[floor, ceiling]`. A
/// seller walks down to its floor, a buyer up to its ceiling; at 1 coin
/// (never-traded default) a buyer climbs one coin per step.
pub fn step_toward(current: i64, sell: bool, floor: i64, ceiling: i64) -> i64 {
    let step = (current / 20).max(1);
    let moved = if sell { current - step } else { current + step };
    clamp_quote(moved, floor.max(1), ceiling.max(floor.max(1)))
}

/// Demand-driven step (2026-11-13, user rule: prices follow supply and
/// demand, 1 coin is only the bootstrap). One step of 5% (at least one
/// coin) per patience window, its direction read from the window:
/// - a sell that filled (even partly) walks UP: buyers take it at this
///   price, so it was cheap; a sell that rested unfilled walks down;
/// - a buy that rested unfilled walks up; a buy that filled walks DOWN:
///   sellers come to it, so it pays too much.
///
/// Always inside `[floor, ceiling]` (a seller's reserve, a buyer's
/// wallet cap) and never under 1. `step_toward` is the unfilled case.
pub fn demand_step(current: i64, sell: bool, filled: bool, floor: i64, ceiling: i64) -> i64 {
    let step = (current / 20).max(1);
    let up = sell == filled;
    let moved = if up { current + step } else { current - step };
    clamp_quote(moved, floor.max(1), ceiling.max(floor.max(1)))
}

/// Opening ask of a new listing (2026-11-13): the current price
/// (`price::current_price`: median of the last five fills clamped by the
/// impact prices of the book, else the book, else 1 coin), or the best resting bid when
/// a buyer already pays more (the listing crosses it at once). No margin
/// on top: the fills of the listing walk it up (`demand_step`).
pub fn opening_ask(reference: i64, best_bid: Option<i64>) -> i64 {
    reference.max(1).max(best_bid.unwrap_or(0))
}

/// Opening bid of a new buy order: the best ask when the ceiling covers
/// it (take it), else the current price, never above the ceiling.
pub fn opening_bid(reference: i64, best_ask: Option<i64>, ceiling: i64) -> i64 {
    let cap = ceiling.max(1);
    match best_ask {
        Some(ask) if ask >= 1 && ask <= cap => ask,
        _ => reference.max(1).min(cap),
    }
}

fn clamp_quote(price: i64, floor: i64, ceiling: i64) -> i64 {
    price.max(floor).min(ceiling).max(1)
}

/// Repricing patience: one 5 % step at most per window of this many ticks
/// (2026-11-14: 2000, was 200; daemon CPU over thousands of resting
/// orders). `market_reprice::PATIENCE_TICKS` is this value.
pub const PATIENCE_TICKS: u64 = 2000;
/// A listing that filled within this many ticks of being listed sold
/// FAST: it was cheap, so the seller's next lot of that item opens at
/// least `FILL_MEMORY_PCT` % of that fill (2026-11-14, user rule 5). One
/// patience window: the fill landed before any walk-down step.
pub const FAST_FILL_TICKS: u64 = PATIENCE_TICKS;
/// Next opening after a fast fill, in percent of that fill's price.
pub const FILL_MEMORY_PCT: i64 = 105;
/// Opening ask of an object nobody traded: this many times the seller's
/// own reserve (2026-11-14, user rule 4), then 5 % steps down to it.
pub const UNTRADED_OPEN_RESERVES: i64 = 2;

/// One of the bot's OWN past fills on an item (its own data only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnFill {
    pub price: i64,
    /// Ticks the listing rested before this fill (from its listing to
    /// the fill, walk steps included); `None` when unknown (a fill read
    /// back from the store after a daemon restart).
    pub rested_ticks: Option<u64>,
}

impl OwnFill {
    /// Sold within `FAST_FILL_TICKS` of its listing.
    pub fn fast(&self) -> bool {
        self.rested_ticks.is_some_and(|ticks| ticks <= FAST_FILL_TICKS)
    }
}

/// Opening ask and reserve floor of a listing (2026-11-14, user rules
/// 1, 4 and 5). `reserve` is the seller's OWN cost of one unit (never a
/// 1-coin default). A traded object opens at its current price (or the
/// best bid when richer); an object nobody traded (price quoted from the
/// book, or the 1-coin default) opens at `max(best bid, 2 x reserve)`,
/// never from that fallback. Never under the reserve; after a fast own
/// fill, at least 105 % of that fill. Returns `(price, floor)` with
/// `floor = reserve`: the walk-down (`demand_step`) stops there.
pub fn seller_open(reference: crate::price::Reference, best_bid: Option<i64>, reserve: i64, last_sell: Option<OwnFill>) -> (i64, i64) {
    let reserve = reserve.max(1);
    let bid = best_bid.unwrap_or(0);
    let base = if reference.traded { reference.price.max(bid) } else { bid.max(reserve.saturating_mul(UNTRADED_OPEN_RESERVES)) };
    let remembered = last_sell.filter(OwnFill::fast).map(|fill| ceil_pct(fill.price, FILL_MEMORY_PCT)).unwrap_or(0);
    (base.max(remembered).max(reserve), reserve)
}

/// Opening ask of FOUND stock (2026-11-14: loot, drops, ground spawns:
/// zero own cost, the fight or the walk paid nothing for it), priced to
/// clear like a player emptying a loot bank: a traded object at its
/// current price (impact-clamped, `current_price`) or the best bid when
/// richer; an untraded one AT the cheapest resting ask (or into a richer
/// bid); with an empty book at `anchor` (the object's high alchemy
/// value, public game data: the price buyers bid gear at, see
/// `gear_plan`) else `2 x floor`.
///
/// Wave 2 (M5, C8, end of the cascade to 1): a seller JOINS the queue,
/// never under the cheapest ask of another seller (each new seller at
/// ask - 1 walked the whole book down to 1). Only a `pressed` seller (a
/// full bank) undercuts, by one relative step (`UNDERCUT_PCT`, at least
/// one coin), or hits a bid at or over its floor. Never under `floor`
/// (the keep value: 1, a caster's alchemy net, an NPC shop's buy
/// price); a fast own fill lifts the lot like `seller_open`. Returns
/// `(price, floor)`: the concession stops there.
pub fn found_open(reference: crate::price::Reference, best_bid: Option<i64>, best_ask: Option<i64>, floor: i64, anchor: Option<i64>, last_sell: Option<OwnFill>, pressed: bool) -> (i64, i64) {
    let floor = floor.max(1);
    let bid = best_bid.unwrap_or(0);
    if pressed && bid >= floor {
        return (bid, floor);
    }
    let ask = best_ask.filter(|ask| *ask >= 1);
    let base = if reference.traded {
        reference.price.max(bid)
    } else {
        match ask {
            Some(ask) => ask.max(bid),
            None if bid >= 1 => bid,
            None => anchor.filter(|value| *value >= 1).unwrap_or(floor.saturating_mul(UNTRADED_OPEN_RESERVES)),
        }
    };
    let joined = match ask {
        Some(ask) if pressed => base.min(ask - undercut_step(ask)),
        Some(ask) => base.max(ask),
        None => base,
    };
    let remembered = if pressed { 0 } else { last_sell.filter(OwnFill::fast).map(|fill| ceil_pct(fill.price, FILL_MEMORY_PCT)).unwrap_or(0) };
    (joined.max(remembered).max(floor), floor)
}

/// Relative undercut of a pressed seller (M5): 1 % of the ask.
pub const UNDERCUT_PCT: i64 = 1;

/// One relative undercut step under `ask` (at least one coin).
pub fn undercut_step(ask: i64) -> i64 {
    ((ask.max(1) * UNDERCUT_PCT + 99) / 100).max(1)
}

/// Opening bid that remembers the buyer's OWN last fill on the item
/// (2026-11-14, user rule 5): a covered ask is taken, else the bid rests
/// at its own last fill price (the reference only when it never bought
/// the item), never above the ceiling.
pub fn remembered_bid(reference: i64, best_ask: Option<i64>, ceiling: i64, last_buy: Option<i64>) -> i64 {
    opening_bid(last_buy.filter(|price| *price >= 1).unwrap_or(reference), best_ask, ceiling)
}

/// Merchant quotes off the book (2026-11-14, user rule 6): buy at the
/// best bid + 1, sell at the best ask - 1, never at reference +/- 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlipQuote {
    /// Buy leg; `None` when the spread does not pay the merchant's time.
    pub buy: Option<i64>,
    /// Sell leg of held stock: never under its floor.
    pub sell: i64,
}

/// `min_margin`: coins per unit the merchant's time costs (its own
/// reserve logic). The buy leg opens only when both sides rest and the
/// spread left between `bid + 1` and `ask - 1` covers that margin; the
/// sell leg rests at `ask - 1`, never under `sell_floor` (what the stock
/// cost it plus the margin, or its own reserve) nor on the best bid, and
/// with no ask at `2 x sell_floor` (the untraded rule).
pub fn flip_quotes(best_bid: Option<i64>, best_ask: Option<i64>, min_margin: i64, sell_floor: i64) -> FlipQuote {
    let floor = sell_floor.max(1);
    let buy = match (best_bid, best_ask) {
        (Some(bid), Some(ask)) if bid >= 1 && (ask - 1) - (bid + 1) >= min_margin.max(1) => Some(bid + 1),
        _ => None,
    };
    let sell = match best_ask {
        Some(ask) if ask >= 2 => (ask - 1).max(floor),
        _ => floor.saturating_mul(UNTRADED_OPEN_RESERVES),
    };
    FlipQuote { buy, sell: sell.max(best_bid.map(|bid| bid + 1).unwrap_or(0)) }
}

fn ceil_pct(price: i64, pct: i64) -> i64 {
    (price.max(0).saturating_mul(pct) + 99) / 100
}

/// Risk caps for merchant bots (fractions of capital/book, item counts).
#[derive(Clone, Debug, PartialEq)]
pub struct MerchantCaps {
    /// Max share of capital in one item (e.g. 0.10).
    pub max_capital_share: f64,
    /// Max resting units of one item.
    pub max_stock_items: i64,
    /// Max share of one side's resting depth (e.g. 0.25 guard rail).
    pub max_book_share: f64,
}

impl MerchantCaps {
    /// Sensible defaults: 10% of capital, 500 units, 25% of the book.
    pub fn standard() -> Self {
        MerchantCaps { max_capital_share: 0.10, max_stock_items: 500, max_book_share: 0.25 }
    }

    /// Capital check: the new position must stay inside its share.
    pub fn check_capital(&self, position_cost: i64, capital: i64) -> Result<(), Error> {
        if capital < 1 {
            return Err(Error::Invalid(format!("bad capital {capital}")));
        }
        if position_cost as f64 > capital as f64 * self.max_capital_share {
            return Err(Error::Rejected(format!(
                "position {position_cost} over capital share on {capital}"
            )));
        }
        Ok(())
    }

    /// Stock check: units of one item stay under the cap.
    pub fn check_stock(&self, units: i64) -> Result<(), Error> {
        if units > self.max_stock_items {
            return Err(Error::Rejected(format!(
                "stock {units} over cap {}",
                self.max_stock_items
            )));
        }
        Ok(())
    }

    /// Book check: the merchant never owns more than its share of a side.
    pub fn check_book(&self, own_qty: i64, side_qty: i64) -> Result<(), Error> {
        if side_qty <= 0 {
            return Ok(());
        }
        if own_qty as f64 > side_qty as f64 * self.max_book_share {
            return Err(Error::Rejected(format!(
                "book share {own_qty}/{side_qty} over cap"
            )));
        }
        Ok(())
    }
}

/// Merchant table pick: widest spread among books with at least
/// `min_volume` recently traded. Returns the item on top, if any.
pub fn pick_table(spreads: &[(i64, i64, i64)]) -> Option<i64> {
    let mut ranked: Vec<(i64, i64, i64)> = spreads.to_vec();
    ranked.sort_by(|left, right| right.1.cmp(&left.1));
    ranked.into_iter().next().map(|row| row.0)
}

#[cfg(test)]
mod step_tests {
    use super::step_toward;

    #[test]
    fn steps_walk_toward_the_bounds() {
        assert_eq!(step_toward(1, false, 1, 50), 2, "a 1-coin bid climbs one coin");
        assert_eq!(step_toward(100, false, 1, 200), 105, "5% up");
        assert_eq!(step_toward(198, false, 1, 200), 200, "capped at the ceiling");
        assert_eq!(step_toward(100, true, 60, 1000), 95, "5% down");
        assert_eq!(step_toward(61, true, 60, 1000), 60, "held at the floor");
        assert_eq!(step_toward(1, true, 1, 1), 1, "never under 1");
    }

    #[test]
    fn demand_steps_follow_fills() {
        use super::demand_step;
        let cap = i64::MAX / 4;
        assert_eq!(demand_step(1, true, true, 1, cap), 2, "a 1-coin ask that sells climbs");
        assert_eq!(demand_step(100, true, true, 1, cap), 105, "a filled ask walks up 5%");
        assert_eq!(demand_step(100, true, true, 1, 102), 102, "never past its cap");
        assert_eq!(demand_step(100, true, false, 60, cap), 95, "an idle ask walks down");
        assert_eq!(demand_step(61, true, false, 60, cap), 60, "held at the reserve");
        assert_eq!(demand_step(10, false, false, 1, 50), 11, "an idle bid walks up");
        assert_eq!(demand_step(50, false, false, 1, 50), 50, "held at the wallet cap");
        assert_eq!(demand_step(40, false, true, 30, 50), 38, "a filled bid walks down");
        assert_eq!(demand_step(30, false, true, 30, 50), 30, "never under its opening");
        assert_eq!(demand_step(1, false, true, 1, 1), 1, "never under 1");
    }

    #[test]
    fn openings_start_at_the_reference() {
        use super::{opening_ask, opening_bid};
        assert_eq!(opening_ask(1, None), 1, "bootstrap: 1 coin");
        assert_eq!(opening_ask(40, None), 40, "no margin over the reference");
        assert_eq!(opening_ask(40, Some(55)), 55, "a richer bid is taken");
        assert_eq!(opening_ask(40, Some(20)), 40);
        assert_eq!(opening_bid(10, Some(12), 20), 12, "covered ask taken");
        assert_eq!(opening_bid(10, Some(30), 20), 10, "ask above the cap: rest at the reference");
        assert_eq!(opening_bid(50, None, 20), 20, "never above the cap");
        assert_eq!(opening_bid(1, None, 0), 1, "never under 1");
    }
}

#[cfg(test)]
mod seller_tests {
    use super::{flip_quotes, remembered_bid, seller_open, FlipQuote, OwnFill};
    use crate::price::Reference;

    const UNTRADED: Reference = Reference { price: 1, traded: false };

    #[test]
    fn untraded_objects_open_at_twice_the_reserve_never_at_one() {
        assert_eq!(seller_open(UNTRADED, None, 6, None), (12, 6), "2 x reserve, walks down to it");
        assert_eq!(seller_open(UNTRADED, Some(20), 6, None), (20, 6), "a richer bid is taken");
        assert_eq!(seller_open(Reference { price: 3, traded: false }, None, 4, None), (8, 4), "a quoted mid never anchors a sale");
        assert_ne!(seller_open(UNTRADED, None, 1, None).0, 1, "never opens at 1");
    }

    #[test]
    fn traded_objects_open_at_the_reference_never_under_the_reserve() {
        let traded = |price| Reference { price, traded: true };
        assert_eq!(seller_open(traded(40), None, 10, None), (40, 10));
        assert_eq!(seller_open(traded(40), Some(55), 10, None), (55, 10));
        // Willow 13 -> 6 -> 2 -> 1: a dumped reference no longer drags
        // the seller under its own cost.
        assert_eq!(seller_open(traded(1), None, 7, None), (7, 7), "held at the reserve");
    }

    #[test]
    fn a_fast_own_fill_lifts_the_next_lot() {
        let traded = Reference { price: 10, traded: true };
        let fast = OwnFill { price: 20, rested_ticks: Some(120) };
        let slow = OwnFill { price: 20, rested_ticks: Some(2500) };
        let unknown = OwnFill { price: 20, rested_ticks: None };
        assert!(fast.fast() && !slow.fast() && !unknown.fast());
        assert_eq!(seller_open(traded, None, 5, Some(fast)), (21, 5), "20 x 1.05");
        assert_eq!(seller_open(traded, None, 5, Some(slow)), (10, 5), "a slow fill says nothing");
        assert_eq!(seller_open(traded, None, 5, Some(unknown)), (10, 5));
        assert_eq!(seller_open(traded, Some(30), 5, Some(fast)), (30, 5), "a richer bid still wins");
    }

    #[test]
    fn bids_open_at_their_own_last_fill() {
        assert_eq!(remembered_bid(1, None, 50, Some(12)), 12, "its own last fill, not the 1-coin reference");
        assert_eq!(remembered_bid(1, None, 10, Some(12)), 10, "never above the ceiling");
        assert_eq!(remembered_bid(4, None, 50, None), 4, "never bought: the reference");
        assert_eq!(remembered_bid(1, Some(9), 50, Some(12)), 9, "a covered ask is taken");
    }

    #[test]
    fn flips_price_off_the_book_with_a_paid_margin() {
        assert_eq!(flip_quotes(Some(10), Some(20), 3, 12), FlipQuote { buy: Some(11), sell: 19 });
        assert_eq!(flip_quotes(Some(10), Some(14), 3, 12), FlipQuote { buy: None, sell: 13 }, "spread 11..13 pays 2 < 3");
        assert_eq!(flip_quotes(Some(10), Some(12), 1, 15), FlipQuote { buy: None, sell: 15 }, "never under its floor");
        assert_eq!(flip_quotes(None, Some(20), 1, 5), FlipQuote { buy: None, sell: 19 }, "no bid: no buy leg");
        assert_eq!(flip_quotes(Some(3), None, 1, 5), FlipQuote { buy: None, sell: 10 }, "no ask: 2 x floor");
        assert_eq!(flip_quotes(Some(1), Some(2), 1, 1), FlipQuote { buy: None, sell: 2 }, "never ref - 1 / ref + 1 at one coin");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoter() -> ZipQuote {
        ZipQuote { margin: 0.10, patience_ticks: 100, floor: 3, ceiling: 20 }
    }

    #[test]
    fn opening_applies_margin_inside_bounds() {
        let quote = quoter();
        assert_eq!(quote.opening(10, true), Ok(11));
        assert_eq!(quote.opening(10, false), Ok(9));
        // Bounds win over the margin.
        assert_eq!(quote.opening(100, true), Ok(20));
        assert_eq!(quote.opening(1, false), Ok(3));
    }

    #[test]
    fn waiting_walks_toward_the_market_not_through_the_floor() {
        let quote = quoter();
        assert_eq!(quote.stepped(11, 50, true), 11, "patient so far");
        assert_eq!(quote.stepped(11, 100, true), 10);
        assert_eq!(quote.stepped(3, 10_000, true), 3, "floor holds");
        assert_eq!(quote.stepped(20, 10_000, false), 20, "ceiling holds");
    }

    #[test]
    fn caps_reject_oversize_positions() {
        let caps = MerchantCaps::standard();
        assert!(caps.check_capital(100, 1000).is_ok());
        assert!(caps.check_capital(101, 1000).is_err());
        assert!(caps.check_stock(500).is_ok());
        assert!(caps.check_stock(501).is_err());
        assert!(caps.check_book(25, 100).is_ok());
        assert!(caps.check_book(26, 100).is_err());
        assert!(caps.check_book(10_000, 0).is_ok(), "empty book has no share");
    }

    #[test]
    fn table_pick_prefers_the_widest_spread() {
        assert_eq!(pick_table(&[(1, 3, 400), (2, 9, 500), (3, 5, 600)]), Some(2));
        assert_eq!(pick_table(&[]), None);
    }
}
