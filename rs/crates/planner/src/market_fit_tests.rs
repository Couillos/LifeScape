//! Market fit: saturation, sale lots, own downstream use, coin rate.

use super::{coin_rate, own_use_items, sale_lot, saturation_factor, BookView, MIN_COIN_RATE, SALE_LOT_MIN};
use crate::ambition::{new_ambition, BotNow, MilestoneStatus, Stamp, Target};

/// Live 2026-11-14: 33 364 willow logs offered at 1 coin, 4 319 traded
/// in a day, no bid: a batch of 180 more is worth a tenth of its price.
/// A demanded object (bids over the asks) keeps its whole value; an
/// object nobody offers is never saturated.
#[test]
fn saturated_products_are_worth_the_share_the_market_absorbs() {
    let willow = BookView { bids: Vec::new(), asks: vec![(1, 33_364)], volume_24h: 4_319 };
    let glut = saturation_factor(Some(&willow), 180.0);
    assert!(glut > 0.1 && glut < 0.15, "{glut}");
    let wanted = BookView { bids: vec![(40, 300)], asks: vec![(45, 50)], volume_24h: 100 };
    assert_eq!(saturation_factor(Some(&wanted), 180.0), 1.0);
    let empty = BookView { bids: Vec::new(), asks: Vec::new(), volume_24h: 0 };
    assert_eq!(saturation_factor(Some(&empty), 500.0), 1.0, "first seller");
    assert_eq!(saturation_factor(None, 500.0), 1.0, "no data, no decision");
    assert!(saturation_factor(Some(&willow), 0.0) > glut, "the batch itself weighs");
}

/// A real player lists what buyers take, not 24 310 feathers at once.
#[test]
fn a_lot_is_what_buyers_take_at_least_an_inventory() {
    let feathers = BookView { bids: vec![(2, 50)], asks: vec![(1, 30_000)], volume_24h: 300 };
    assert_eq!(sale_lot(24_310, Some(&feathers)), 350);
    assert_eq!(sale_lot(24_310, None), SALE_LOT_MIN);
    assert_eq!(sale_lot(10, Some(&feathers)), 10, "never more than the stack");
}

/// The bot's time: the best published profit it can work at its levels.
#[test]
fn coin_rate_reads_the_methods_the_bot_can_work() {
    let rows = vec![
        ("woodcut".to_string(), 36, 16_732.0),
        ("cook".to_string(), 1, 3_000.0),
        ("market".to_string(), 1, 90_000.0),
    ];
    let cook = |skill: &str| if skill == "cooking" { 10 } else { 1 };
    assert!((coin_rate(&rows, &cook) - 0.5).abs() < 1e-9, "woodcut 36 out of reach, flips never count");
    let lumberjack = |skill: &str| if skill == "woodcutting" { 40 } else { 1 };
    assert!(coin_rate(&rows, &lumberjack) > 2.7);
    assert_eq!(coin_rate(&[], &cook), MIN_COIN_RATE, "time is never free");
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
