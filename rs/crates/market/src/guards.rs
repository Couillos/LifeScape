#![forbid(unsafe_code)]
//! Guard rails: an order is rejected loudly when its price is not a
//! positive coin count or it is uncovered. No band around a reference
//! (user rule 2026-11-15): supply and demand alone drive prices.

use crate::error::Error;
use crate::Side;

/// Price check: at least 1 coin, nothing else (no reference band).
pub fn check_price(price: i64) -> Result<(), Error> {
    if price < 1 {
        return Err(Error::Invalid(format!("bad price {price}")));
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
    fn any_positive_price_passes_supply_and_demand_decide() {
        assert!(check_price(1).is_ok());
        assert!(check_price(1_000_000).is_ok());
        assert!(matches!(check_price(0), Err(Error::Invalid(_))));
        assert!(matches!(check_price(-3), Err(Error::Invalid(_))));
    }

    #[test]
    fn coverage_needs_the_goods_or_the_coins() {
        assert!(check_coverage(Side::Sell, 10, 5, 10, 0).is_ok());
        assert!(check_coverage(Side::Sell, 11, 5, 10, 0).is_err());
        assert!(check_coverage(Side::Buy, 10, 5, 0, 50).is_ok());
        assert!(check_coverage(Side::Buy, 10, 5, 0, 49).is_err());
    }
}
