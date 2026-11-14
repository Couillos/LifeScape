//! Seller sanity (live 2026-11-14): one bronze scimitar gear bid filled
//! at 2747, then 21 scimitars listed at 1149-2480 with no buyer. The
//! asks must come down with patience, never stay anchored on the
//! outlier fill (the 5× band is unchanged).

use crate::{current_price, demand_step, seller_open, LastFill, Reference};

/// The live asks, cheapest first (one unit each).
fn scimitar_asks() -> Vec<(i64, i64)> {
    (0..21).map(|step| (1149 + step * (2480 - 1149) / 20, 1)).collect()
}

#[test]
fn an_outlier_fill_never_anchors_the_asks() {
    let fills = [LastFill { price: 2747, quantity: 1 }];
    let asks = scimitar_asks();
    // The current price: the one fill clamped by the impact ask (1 unit).
    let reference = current_price(&fills, &[], &asks);
    assert_eq!(reference, Reference { price: 1149, traded: true }, "the cheapest real ask is the price of the moment");
    // A new lot opens at that price, not at the fill.
    let (open, floor) = seller_open(reference, None, 40, None);
    assert_eq!((open, floor), (1149, 40));
    // Resting unfilled, every ask walks down 5 % a window to its reserve:
    // after 60 windows (120 000 ticks) the dearest is under 140.
    let reserve = 40;
    let mut walked: Vec<i64> = asks.iter().map(|(price, _)| *price).collect();
    for _window in 0..60 {
        for price in walked.iter_mut() {
            *price = demand_step(*price, true, false, reserve, i64::MAX / 4);
        }
    }
    assert!(walked.iter().all(|price| *price < 140 && *price >= reserve), "{walked:?}");
    // And the reference follows the book down with them.
    let book: Vec<(i64, i64)> = walked.iter().map(|price| (*price, 1)).collect();
    assert!(current_price(&fills, &[], &book).price < 140);
}
