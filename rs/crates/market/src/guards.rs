#![forbid(unsafe_code)]
//! Guard rails: every order is rejected loudly when it is too far from
//! the reference price or uncovered. Sells may fall to 1 coin; buys must
//! sit inside 0.2x-5x of the reference when one is known.

use crate::error::Error;
use crate::Side;

/// Price band check. `reference` is `None` until the object has traded
/// (a quote-only or default 1-coin reference never caps an order, or
/// nobody could ever bid an untraded object above 5 coins); without one
/// only the 1-coin floor applies.
pub fn check_price(side: Side, price: i64, reference: Option<i64>) -> Result<(), Error> {
    if price < 1 {
        return Err(Error::Invalid(format!("bad price {price}")));
    }
    let Some(anchor) = reference else {
        return Ok(());
    };
    if anchor < 1 {
        return Err(Error::Invalid(format!("bad reference {anchor}")));
    }
    // Integer math: price > 5x  <=>  price/anchor > 5 without floats.
    if price > anchor.saturating_mul(5) {
        return Err(Error::Rejected(format!("price {price} above 5x reference {anchor}")));
    }
    if side == Side::Buy && price.saturating_mul(5) < anchor {
        return Err(Error::Rejected(format!("buy price {price} below 0.2x reference {anchor}")));
    }
    Ok(())
}

/// Coverage check against caller-supplied balances (the service feeds live
/// holdings for connected bots, its own ledger for offline tests).
pub fn check_coverage(
    side: Side,
    quantity: i64,
    price: i64,
    have_item: i64,
    have_coins: i64,
) -> Result<(), Error> {
    if quantity <= 0 {
        return Err(Error::Invalid(format!("bad quantity {quantity}")));
    }
    match side {
        Side::Sell => {
            if have_item < quantity {
                return Err(Error::Rejected(format!(
                    "seller holds {have_item}, needs {quantity}"
                )));
            }
        }
        Side::Buy => {
            let cost = (quantity as i128) * (price as i128);
            if (have_coins as i128) < cost {
                return Err(Error::Rejected(format!(
                    "buyer holds {have_coins} coins, needs {cost}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_pin_buys_but_let_sells_fall() {
        assert!(check_price(Side::Buy, 500, Some(100)).is_ok());
        assert!(check_price(Side::Sell, 1, Some(100)).is_ok());
        assert!(matches!(
            check_price(Side::Buy, 501, Some(100)),
            Err(Error::Rejected(_))
        ));
        assert!(matches!(
            check_price(Side::Sell, 501, Some(100)),
            Err(Error::Rejected(_))
        ));
        // 19 is below 0.2x of 100 for a buy, fine for a sell.
        assert!(matches!(
            check_price(Side::Buy, 19, Some(100)),
            Err(Error::Rejected(_))
        ));
        assert!(check_price(Side::Sell, 19, Some(100)).is_ok());
        assert!(check_price(Side::Buy, 20, Some(100)).is_ok());
        assert!(check_price(Side::Buy, 7, None).is_ok());
    }

    #[test]
    fn coverage_needs_the_goods_or_the_coins() {
        assert!(check_coverage(Side::Sell, 10, 5, 10, 0).is_ok());
        assert!(check_coverage(Side::Sell, 11, 5, 10, 0).is_err());
        assert!(check_coverage(Side::Buy, 10, 5, 0, 50).is_ok());
        assert!(check_coverage(Side::Buy, 10, 5, 0, 49).is_err());
    }
}
