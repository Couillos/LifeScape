//! Market fit: saturation, sale lots, own downstream use, coin rate.

use super::{own_use_items, sale_lot, saturation_factor, BookView, SALE_LOT_MIN};
use crate::ambition::{new_ambition, BotNow, MilestoneStatus, Stamp, Target};

/// Live 2026-11-14: 33 364 willow logs offered at 1 coin, 4 319 traded
/// in a day, no bid: a batch of 180 more is worth a tenth of its price.
/// A demanded object (bids over the asks) keeps its whole value; an
/// object nobody offers is never saturated.
#[test]
fn saturated_products_are_worth_the_share_the_market_absorbs() {
    let willow = BookView { bids: Vec::new(), asks: vec![(1, 33_364)], volume_24h: 4_319, ..BookView::default() };
    let glut = saturation_factor(Some(&willow), 180.0);
    assert!(glut > 0.1 && glut < 0.15, "{glut}");
    let wanted = BookView { bids: vec![(40, 300)], asks: vec![(45, 50)], volume_24h: 100, ..BookView::default() };
    assert_eq!(saturation_factor(Some(&wanted), 180.0), 1.0);
    let empty = BookView { bids: Vec::new(), asks: Vec::new(), volume_24h: 0, ..BookView::default() };
    assert_eq!(saturation_factor(Some(&empty), 500.0), 1.0, "first seller");
    assert_eq!(saturation_factor(None, 500.0), 1.0, "no data, no decision");
    assert!(saturation_factor(Some(&willow), 0.0) > glut, "the batch itself weighs");
}

/// A real player lists what buyers take, not 24 310 feathers at once.
#[test]
fn a_lot_is_what_buyers_take_at_least_an_inventory() {
    let feathers = BookView { bids: vec![(2, 50)], asks: vec![(1, 30_000)], volume_24h: 300, ..BookView::default() };
    assert_eq!(sale_lot(24_310, Some(&feathers)), 350);
    assert_eq!(sale_lot(24_310, None), SALE_LOT_MIN);
    assert_eq!(sale_lot(10, Some(&feathers)), 10, "never more than the stack");
}

/// A firemaker keeps the logs its own climb burns, a smith the bars and
/// ores of its quota, a priest its bones; a plain woodcutter keeps none.
#[test]
fn own_milestones_keep_what_they_consume() {
    let level_of = |skill: &str| match skill {
        "woodcutting" => 35,
        "firemaking" => 30,
        "smithing" => 10,
        "mining" => 12,
        _ => 1,
    };
    let now = BotNow { level_of: &level_of, coins: 0, held: &|_| 0 };
    let stamp = Stamp { tick: 0, secs: 0 };
    let firemaker = new_ambition("firemaker", "random", &now, 3, &[], stamp).expect("firemaker");
    let keep = own_use_items(&firemaker);
    assert!(keep.iter().any(|item| item == "willow_logs"), "firemaking 30 → 45 burns willows: {keep:?}");
    let smith = new_ambition("smith", "random", &now, 3, &[], stamp).expect("smith");
    let keep = own_use_items(&smith);
    assert!(keep.iter().any(|item| item == "bronze_bar"), "{keep:?}");
    assert!(keep.iter().any(|item| item == "copper_ore") && keep.iter().any(|item| item == "tin_ore"), "{keep:?}");
    let woodcutter = new_ambition("woodcutter", "random", &now, 3, &[], stamp).expect("woodcutter");
    assert!(own_use_items(&woodcutter).is_empty(), "a woodcutter sells its logs: {:?}", own_use_items(&woodcutter));
    let mut done = firemaker.clone();
    for stone in &mut done.milestones {
        stone.status = MilestoneStatus::Done;
    }
    assert!(own_use_items(&done).is_empty(), "done milestones keep nothing");
    assert!(firemaker.milestones.iter().any(|stone| matches!(&stone.target, Target::Level { skill, .. } if skill == "firemaking")));
}

/// Live 2026-11-18: nathan_t held 564 mithril full helms against one
/// trade in 30 hours; 529 kiteshields against ~61 a day. Overstock: the
/// free stock beyond the bids plus two days of volume (at least one
/// inventory). Willow logs sold by the thousand a day are not.
#[test]
fn stock_beyond_two_days_of_demand_is_overstock() {
    use super::overstocked;
    let helms = BookView { bids: vec![(823, 1)], asks: vec![(858, 60)], volume_24h: 1, ..BookView::default() };
    assert!(overstocked(564, &helms));
    let kites = BookView { bids: vec![(1131, 2)], asks: Vec::new(), volume_24h: 61, ..BookView::default() };
    assert!(overstocked(529, &kites));
    assert!(!overstocked(120, &kites), "two days of kites sell");
    assert!(!overstocked(SALE_LOT_MIN, &BookView::default()), "one inventory never is");
    assert!(overstocked(SALE_LOT_MIN + 1, &BookView::default()), "nothing traded: past one inventory");
}

/// Point 2 (user 2026-11-18): nathan_t smithed 564 mithril full helms in
/// one money lot (`--use mithril_bar 3274`) for a market that took one a
/// day. A stock lot runs only the actions whose product the market
/// absorbs: bids plus two days of volume, less what rests offered and
/// what the bot already holds.
#[test]
fn a_stock_lot_never_makes_past_what_the_market_absorbs() {
    use super::absorbable_actions;
    let helms = BookView { bids: vec![(823, 1)], asks: vec![(858, 60)], volume_24h: 1, ..BookView::default() };
    assert_eq!(absorbable_actions(&helms, 0, 1), 0, "60 already offered against 3 absorbed");
    let bars = BookView { bids: vec![(60, 100)], asks: vec![(62, 50)], volume_24h: 400, ..BookView::default() };
    assert_eq!(absorbable_actions(&bars, 0, 1), 850, "100 bid + 800 in two days - 50 offered");
    assert_eq!(absorbable_actions(&bars, 300, 1), 550, "its own 300 sell first");
    assert_eq!(absorbable_actions(&bars, 0, 5), 170, "five units per action");
}
