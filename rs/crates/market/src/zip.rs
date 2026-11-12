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

fn clamp_quote(price: i64, floor: i64, ceiling: i64) -> i64 {
    price.max(floor).min(ceiling).max(1)
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
