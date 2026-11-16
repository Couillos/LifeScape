//! Butin à coût nul (live 2026-11-14, frank_lane, guerriers d'Al
//! Kharid) : 41 dagues de fer, 88 heaumes moyens de bronze, 838 flèches
//! de bronze en banque, une seule dague listée à 628 pour une valeur de
//! ~30. Le butin s'ouvre au prix qui écoule. Vague 2 (M5) : un vendeur
//! rejoint la file au meilleur ask, il ne sous-cote plus d'une pièce
//! (cascade à 1) ; seul un vendeur pressé sous-cote d'un pas relatif.

use crate::{current_price, found_open, LastFill, OwnFill, Reference};

const UNTRADED_EMPTY: Reference = Reference { price: 1, traded: false };

#[test]
fn traded_loot_opens_at_the_current_price_never_at_an_outlier() {
    // Bronze arrows: five fills at 2, asks 1 (7 units), 2 (14, 28, 451).
    let fills: Vec<LastFill> = [28, 43, 159, 77, 25].iter().map(|quantity| LastFill { price: 2, quantity: *quantity }).collect();
    let asks = vec![(1, 7), (2, 14), (2, 28), (2, 451)];
    let reference = current_price(&fills, &[], &asks);
    assert_eq!(reference, Reference { price: 2, traded: true }, "seven units at 1 never price a lot of 43");
    assert_eq!(found_open(reference, None, Some(1), 1, Some(1), None, false), (2, 1), "the current price, floor 1");
    assert_eq!(found_open(reference, Some(3), Some(1), 1, None, None, false), (3, 1), "a richer bid is taken");
}

#[test]
fn untraded_loot_joins_the_cheapest_ask_or_opens_at_the_alch_value() {
    // Iron daggers: nobody traded them, asks rest at 18, 22, 23, 28.
    let reference = Reference { price: 18, traded: false };
    assert_eq!(found_open(reference, None, Some(18), 1, Some(21), None, false), (18, 1), "joins the cheapest seller");
    // Fur: an empty book opens at its high alchemy value.
    assert_eq!(found_open(UNTRADED_EMPTY, None, None, 1, Some(12), None, false), (12, 1));
    assert_eq!(found_open(UNTRADED_EMPTY, None, None, 1, None, None, false), (2, 1), "junk without a value: 2 x floor");
    assert_eq!(found_open(UNTRADED_EMPTY, Some(9), None, 1, Some(12), None, false), (9, 1), "a resting bid clears now");
    // A caster's alchemy net (or a shop's buy price) is the floor.
    assert_eq!(found_open(reference, None, Some(18), 20, Some(21), None, false), (20, 20));
    // A fast own fill still lifts the next lot.
    let fast = OwnFill { price: 30, rested_ticks: Some(10) };
    assert_eq!(found_open(reference, None, Some(18), 1, None, Some(fast), false), (32, 1));
}

/// M5 test: `found_open` is never under the best ask of another seller
/// unless the seller is pressed (a full bank), and then by one relative
/// step only, never under its floor.
#[test]
fn found_open_never_undercuts_the_best_ask() {
    for (ask, reference) in [(2, UNTRADED_EMPTY), (18, Reference { price: 18, traded: false }), (40, Reference { price: 30, traded: true }), (500, Reference { price: 520, traded: true })] {
        for floor in [1, 5] {
            let (price, _) = found_open(reference, None, Some(ask), floor, Some(7), None, false);
            assert!(price >= ask, "ask {ask} ref {reference:?}: opened {price}");
        }
    }
    assert_eq!(found_open(UNTRADED_EMPTY, None, Some(500), 1, None, None, true), (495, 1), "pressed: 1 % under");
    assert_eq!(found_open(UNTRADED_EMPTY, None, Some(9), 1, None, None, true), (8, 1), "at least one coin");
    assert_eq!(found_open(UNTRADED_EMPTY, None, Some(9), 9, None, None, true), (9, 9), "never under its floor");
    assert_eq!(found_open(UNTRADED_EMPTY, Some(6), Some(9), 5, None, None, true), (6, 5), "pressed: hits a bid over its floor");
}

/// Fifty loot sellers arriving one after the other on an untraded
/// object: the first opens at its anchor, every later one joins it. No
/// cascade toward 1.
#[test]
fn arriving_loot_sellers_never_cascade() {
    let mut best_ask: Option<i64> = None;
    for _seller in 0..50 {
        let (price, _) = found_open(UNTRADED_EMPTY, None, best_ask, 1, Some(21), None, false);
        best_ask = Some(best_ask.map_or(price, |ask| ask.min(price)));
    }
    assert_eq!(best_ask, Some(21));
}
