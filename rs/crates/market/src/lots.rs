#![forbid(unsafe_code)]
//! Quotation by lot (plan prix lot D, user decision Q3 2026-11-18): an
//! object worth a few coins was quoted per unit, so a price step was ±50 %
//! at 1-2 coins and nothing moved below 1. Such an object is quoted for a
//! LOT of units, a size fixed by its game value (a book never changes
//! unit): the price of an order, a deal, an execution and a candle is in
//! coins per lot; quantities stay in units, whole lots only. The brain
//! keeps per-unit prices: the market service converts at its boundary.

/// Unit price at or under which an object is quoted per 100 units.
pub const HUNDRED_MAX_PRICE: i64 = 5;
/// Unit price at or under which an object is quoted per 10 units.
pub const TEN_MAX_PRICE: i64 = 20;

/// Lot size of an object worth `price` coins a unit (its observed market
/// price when it traded, else its game value: bones are worth 1 in the
/// game and trade at 50): 100 at 5 coins or less, 10 up to 20, else 1.
/// Decided once per book (`Store::set_item_lot`), never moved by a later
/// price.
pub fn lot_for_price(price: i64) -> i64 {
    if price <= HUNDRED_MAX_PRICE {
        100
    } else if price <= TEN_MAX_PRICE {
        10
    } else {
        1
    }
}

/// Coins `quantity` units cost at `price` per lot of `lot` units (whole
/// lots: exact; a stray remainder rounds up, never a free unit).
pub fn lot_coins(quantity: i64, price: i64, lot: i64) -> i64 {
    let lot = lot.max(1);
    (quantity.saturating_mul(price) + lot - 1).div_euclid(lot)
}

/// Units of whole lots in `quantity` (a listing never offers a stray
/// remainder).
pub fn whole_lots_down(quantity: i64, lot: i64) -> i64 {
    let lot = lot.max(1);
    quantity.max(0) / lot * lot
}

/// Units of whole lots covering `quantity` (a purchase buys the lot that
/// covers its need).
pub fn whole_lots_up(quantity: i64, lot: i64) -> i64 {
    let lot = lot.max(1);
    (quantity.max(0) + lot - 1) / lot * lot
}

/// Price of one lot from a per-unit price (the brain's coins per unit).
pub fn lot_price(unit_price: i64, lot: i64) -> i64 {
    unit_price.max(0).saturating_mul(lot.max(1))
}

/// Per-unit price of a lot price, rounded to the nearest coin, at least 1
/// for a positive price (the brain's integer view).
pub fn unit_price(price: i64, lot: i64) -> i64 {
    let lot = lot.max(1);
    if price <= 0 {
        return price;
    }
    ((price + lot / 2) / lot).max(1)
}

/// Exact per-unit price of a lot price (valuations, guides).
pub fn unit_price_exact(price: i64, lot: i64) -> f64 {
    price as f64 / lot.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live 2026-11-18: water runes at 1-2 per 100; raw sardines at 2
    /// per 100; logs at 12 per 10; bones at 67 per unit.
    #[test]
    fn the_lot_follows_the_unit_price() {
        assert_eq!(lot_for_price(2), 100);
        assert_eq!(lot_for_price(5), 100);
        assert_eq!(lot_for_price(12), 10);
        assert_eq!(lot_for_price(20), 10);
        assert_eq!(lot_for_price(21), 1);
        assert_eq!(lot_for_price(67), 1);
    }

    /// 300 air runes at 140 per 100 cost 420; a stray unit rounds up.
    #[test]
    fn coins_follow_whole_lots() {
        assert_eq!(lot_coins(300, 140, 100), 420);
        assert_eq!(lot_coins(28, 1_241, 1), 34_748, "unit quotation unchanged");
        assert_eq!(lot_coins(150, 140, 100), 210);
        assert_eq!(lot_coins(1, 140, 100), 2, "a stray unit rounds up");
        assert_eq!(whole_lots_down(2_881, 100), 2_800);
        assert_eq!(whole_lots_up(5, 100), 100);
        assert_eq!(whole_lots_up(0, 100), 0);
        assert_eq!(lot_price(2, 100), 200);
        assert_eq!(unit_price(140, 100), 1);
        assert_eq!(unit_price(150, 100), 2);
        assert_eq!(unit_price(30, 100), 1, "never 0 for a positive price");
        assert_eq!(unit_price(1_241, 1), 1_241);
        assert!((unit_price_exact(140, 100) - 1.4).abs() < 1e-9);
    }
}
