//! Magic training on a fabricated world (user 2026-11-20: « si le bot
//! veut s'entraîner alors il faut privilégier l'xp/coût »): the pick
//! weighs the runes a line burns at what they cost to get NOW, a rune
//! nobody sells unavailable, never free.
//!
//! Live 2026-11-20: every mage trained by crumble undead on the
//! Wilderness skeletons (2.3 XP a tick, its drops worth more than its
//! runes at the keepers' price) while the chaos runes traded at ~2 000
//! coins on the market (2 on the asks), the keepers emptied by the
//! fleet: "out of runes for crumble_undead … chaosrune", 79 blocks a
//! day, the fleet's magic time from ~6 % to 0.3 %.

use super::{answer, Fixture};
use crate::guide_feasible::{feasible_for, with_filter_bot, Feasibility};
use crate::guide_model::{GuideFilter, Quote, Supply, TrainingRow};
use crate::guide_training::training_guide;
use crate::model::{Archetype, Profile};

const EARTH_STRIKE: &str = "fight:magic/earth_strike/ladder";
const CRUMBLE: &str = "fight:magic/crumble_undead/wild_skeletons";

/// Earth strike on the ladder (cheap runes, slow) and crumble undead on the
/// skeletons (fast, a chaos rune a cast, rich drops), a window each.
fn magic_world() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.yields.insert(
        EARTH_STRIKE,
        answer(
            &[("mindrune", "rune", -600.0), ("earthrune", "rune", -1200.0), ("airrune", "rune", -600.0), ("cowhide", "loot", 60.0)],
            &[("cast", "earth_strike", 600.0), ("damage", "cow", 1500.0), ("kill", "cow", 60.0)],
        ),
    );
    fixture.yields.insert(
        CRUMBLE,
        answer(
            &[("chaosrune", "rune", -900.0), ("airrune", "rune", -1800.0), ("earthrune", "rune", -1800.0), ("steel_platebody", "loot", 40.0)],
            &[("cast", "crumble_undead", 900.0), ("damage", "skeleton", 3000.0), ("kill", "skeleton", 120.0)],
        ),
    );
    fixture.prices.extend([("mindrune", 1.0), ("earthrune", 1.0), ("airrune", 1.0), ("cowhide", 100.0), ("steel_platebody", 1500.0)]);
    // The keepers' static window (1 000 base units, then the restock),
    // what the server data says they stock when nobody bought.
    fixture.shop_ladders.insert("chaosrune", vec![(15.0, 1000.0), (94.0, 60.0)]);
    fixture
}

/// The live chaos rune: traded far above every keeper's price (the
/// fills prove the keepers sold out), two units on the asks.
fn drained_chaos() -> Quote {
    Quote { price: 2000.0, traded: true, median: 2000.0, fills: 5, fill_units: 60.0, asks: vec![(1717.0, 2.0)], ..Quote::default() }
}

fn mage(level: u8) -> GuideFilter {
    GuideFilter { skill: Some("magic".to_string()), level: Some(level), ..GuideFilter::default() }
}

/// The brain's pick (`brain_train::pick_from_guide`): the lines a bot
/// works now, scored, best first.
fn ranked_now(rows: &[TrainingRow], filter: &GuideFilter) -> Vec<String> {
    let profile = Profile::new(false, 0.65, 0.5, 7, Archetype::Producer).with_thrift(0.5);
    let class = |row: &TrainingRow| with_filter_bot(filter, |bot| matches!(feasible_for(row, bot), Feasibility::Feasible).then_some(0u8));
    crate::train_score::rank_classed(rows, &profile, Some(0.5), &class).into_iter().map(|line| format!("{} {:?}", line.row.method, line.row.supply)).collect()
}

/// The chaos runes the market prices at ~2 000 and the keepers no longer
/// hold: crumble undead is out of reach (2 runes on the asks for a
/// window of 900) and dear; the earth strike on the ladder trains.
#[test]
fn a_mage_trains_with_runes_it_can_get_never_with_drained_chaos_runes() {
    let mut fixture = magic_world();
    fixture.quotes.insert("chaosrune", drained_chaos());
    let filter = mage(45);
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let ranked = ranked_now(&rows, &filter);
    assert!(ranked.first().is_some_and(|line| line.starts_with(EARTH_STRIKE)), "{ranked:?}");
    assert!(!ranked.iter().any(|line| line.starts_with(CRUMBLE)), "crumble undead never feasible on drained chaos runes: {ranked:?}");
    let crumble = rows.iter().find(|row| row.method == CRUMBLE && row.supply != Supply::Quest).expect("crumble line listed");
    let chaos = crumble.flows.iter().find(|line| line.item == "chaosrune").expect("chaos line");
    assert!(chaos.unit_price >= 1700.0, "the chaos rune costs what the market asks now: {chaos:?}");
}

/// The keepers stocked (the market trades the chaos rune at their
/// price): crumble undead's runes cost little, the faster line trains.
#[test]
fn stocked_keepers_keep_crumble_undead_open() {
    let mut fixture = magic_world();
    fixture.quotes.insert("chaosrune", Quote { price: 20.0, traded: true, median: 20.0, fills: 5, fill_units: 500.0, ..Quote::default() });
    let filter = mage(45);
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let ranked = ranked_now(&rows, &filter);
    assert!(ranked.first().is_some_and(|line| line.starts_with(CRUMBLE)), "{ranked:?}");
}
