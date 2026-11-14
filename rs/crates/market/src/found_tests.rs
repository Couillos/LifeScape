//! Butin à coût nul (live 2026-11-14, frank_lane, guerriers d'Al
//! Kharid) : 41 dagues de fer, 88 heaumes moyens de bronze, 838 flèches
//! de bronze en banque, une seule dague listée à 628 pour une valeur de
//! ~30. Le butin s'ouvre au prix qui écoule, puis descend avec la
//! patience jusqu'au plancher (1, ou le net d'alchimie d'un lanceur).

use crate::{current_price, demand_step, found_open, LastFill, OwnFill, Reference};

const UNTRADED_EMPTY: Reference = Reference { price: 1, traded: false };

#[test]
fn traded_loot_opens_at_the_current_price_never_at_an_outlier() {
    // Bronze arrows: five fills at 2, asks 1 (7 units), 2 (14, 28, 451).
    let fills: Vec<LastFill> = [28, 43, 159, 77, 25].iter().map(|quantity| LastFill { price: 2, quantity: *quantity }).collect();
    let asks = vec![(1, 7), (2, 14), (2, 28), (2, 451)];
    let reference = current_price(&fills, &[], &asks);
    assert_eq!(reference, Reference { price: 2, traded: true }, "seven units at 1 never price a lot of 43");
    assert_eq!(found_open(reference, None, Some(1), 1, Some(1), None), (2, 1), "the current price, floor 1");
    assert_eq!(found_open(reference, Some(3), Some(1), 1, None, None), (3, 1), "a richer bid is taken");
}

#[test]
fn untraded_loot_undercuts_the_cheapest_ask_or_opens_at_the_alch_value() {
    // Iron daggers: nobody traded them, asks rest at 18, 22, 23, 28.
    let reference = Reference { price: 18, traded: false };
    assert_eq!(found_open(reference, None, Some(18), 1, Some(21), None), (17, 1), "just under the cheapest seller");
    // Fur: an empty book opens at its high alchemy value.
    assert_eq!(found_open(UNTRADED_EMPTY, None, None, 1, Some(12), None), (12, 1));
    assert_eq!(found_open(UNTRADED_EMPTY, None, None, 1, None, None), (2, 1), "junk without a value: 2 x floor");
    assert_eq!(found_open(UNTRADED_EMPTY, Some(9), None, 1, Some(12), None), (9, 1), "a resting bid clears now");
    // A caster's alchemy net is the floor: never under it.
    assert_eq!(found_open(reference, None, Some(18), 20, Some(21), None), (20, 20));
    // A fast own fill still lifts the next lot.
    let fast = OwnFill { price: 30, rested_ticks: Some(10) };
    assert_eq!(found_open(reference, None, Some(18), 1, None, Some(fast)), (32, 1));
}

/// Steel arrow listed 1 @729 against asks at 9: a found lot opens at 8
/// and an unsold ask walks down to 1 with patience, never stays put.
#[test]
fn unsold_loot_walks_down_to_its_floor() {
    let (mut price, floor) = found_open(Reference { price: 9, traded: false }, None, Some(9), 1, Some(21), None);
    assert_eq!(price, 8);
    for _window in 0..20 {
        price = demand_step(price, true, false, floor, i64::MAX / 4);
    }
    assert_eq!(price, 1, "eight 1-coin steps reach the floor");
}
