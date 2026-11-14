//! What the market absorbs of a lot, real trips, the training share.

use super::{absorbed_lot, leg_site, training_share, walk_ticks, VARROCK_FOUNTAIN};
use crate::market_fit::BookView;

#[test]
fn bids_above_cost_minus_pending_supply_split_in_two() {
    let view = BookView { bids: vec![(300, 4), (200, 4), (50, 10)], asks: vec![(320, 2)], ..BookView::default() };
    let lot = absorbed_lot(&view, 100.0, 0, None, 2, 100).expect("pays");
    // 8 bids above 100, 2 asks within 125 % of 300 already on their way.
    assert_eq!((lot.count, lot.from_bids), (3, 3), "{lot:?}");
    assert!((lot.revenue - 900.0).abs() < 1e-6, "{lot:?}");
    assert_eq!(absorbed_lot(&view, 400.0, 0, None, 2, 100), None, "nothing pays more than 400");
    assert_eq!(absorbed_lot(&view, 100.0, 6, None, 2, 100), None, "its own stock answers them");
}

#[test]
fn sales_at_the_current_price_join_the_bids() {
    let view = BookView { bids: vec![(90, 2)], asks: vec![(70, 4)], volume_24h: 40, current: Some(80), claimed: 0 };
    let lot = absorbed_lot(&view, 10.0, 0, None, 1, 100).expect("pays");
    // 2 bids − 4 pending asks (≤ 112) = 0; sales: 20 of the day − 4 asks.
    assert_eq!((lot.from_bids, lot.from_sales), (0, 16), "{lot:?}");
    let claimed = BookView { claimed: 10, ..view.clone() };
    assert_eq!(absorbed_lot(&claimed, 10.0, 0, None, 1, 100).map(|lot| lot.count), Some(6), "claims are supply on its way");
    let dear = absorbed_lot(&view, 85.0, 0, None, 1, 100);
    assert_eq!(dear, None, "the current price does not pay a unit of 85");
}

#[test]
fn the_alchemy_floor_takes_what_the_market_leaves() {
    let view = BookView { bids: vec![(120, 2)], ..BookView::default() };
    let lot = absorbed_lot(&view, 30.0, 0, Some(60.0), 2, 100).expect("alched");
    assert_eq!((lot.count, lot.from_bids, lot.from_alch), (28, 1, 27), "{lot:?}");
    assert!((lot.revenue - (120.0 + 27.0 * 60.0)).abs() < 1e-6, "{lot:?}");
    assert_eq!(absorbed_lot(&view, 70.0, 0, Some(60.0), 2, 100).map(|lot| lot.from_alch), Some(0), "an alch under cost never counts");
}

#[test]
fn training_is_free_when_planned_else_one_lot_of_four() {
    assert_eq!(training_share(40, 27), 0.0);
    assert_eq!(training_share(0, 27), 0.25);
    assert_eq!(training_share(26, 27), 0.25);
}

/// Smithing happens at the anvil nearest to where the product goes
/// (Varrock west, by the fountain), a few dozen ticks; a furnace for the
/// bars is far (none in Varrock).
#[test]
fn legs_happen_at_real_sites() {
    let anvil = leg_site("smith:iron_full_helm", VARROCK_FOUNTAIN, 3, false).expect("an anvil");
    assert!(walk_ticks(anvil.site, VARROCK_FOUNTAIN) < 60.0, "{anvil:?}");
    assert_eq!(anvil.fare, 0.0);
    let furnace = leg_site("smelt:iron_bar", anvil.site, 3, false).expect("a furnace");
    assert!(walk_ticks(furnace.site, anvil.site) > 60.0, "no furnace in Varrock: {furnace:?}");
}
