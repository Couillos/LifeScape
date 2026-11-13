//! Ambition repairs (review 2026-11-13): the demand loop on stock already
//! held, unprofitable bids, empty draws, ladder extension for the sole
//! holder of a group, coverage repair, the revision clock key.

use super::{
    achieved, ambition_from_value, ambition_to_value, extend, new_ambition, new_ambition_extended, open_count, refresh, AmbitionError, BotNow,
    MilestoneStatus, Stamp, Target, AMBITION_CHANGE_TICKS, GENERALIST, MAX_DONE_KEPT,
};
use crate::ambition_catalog::{DemandItem, MERCHANT};
use crate::ambition_fleet::{coverage_repair, FleetBot};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn fresh(_: &str) -> u8 {
    1
}

fn nothing_held(_: &str) -> i64 {
    0
}

fn now<'view>(level_of: &'view dyn Fn(&str) -> u8, coins: i64, held: &'view dyn Fn(&str) -> i64) -> BotNow<'view> {
    BotNow { level_of, coins, held }
}

fn bid(item: &str, margin: Option<f64>) -> Vec<DemandItem> {
    vec![DemandItem { item: item.to_string(), weight: 500.0, unmet: 100, margin }]
}

fn demand_stones(ambition: &super::Ambition) -> usize {
    ambition.milestones.iter().filter(|stone| stone.origin.starts_with("demand:")).count()
}

/// Finding 1: a cook holding 100 shrimp (protected food) and an open bid
/// for 100 shrimp looped demand milestone / done / reinsert forever.
/// Stock already held never makes a demand milestone; without the stock
/// one is made, and once it is done the stock it made holds it back.
#[test]
fn stock_already_held_never_makes_a_demand_milestone() {
    let stocked = |item: &str| if item == "shrimp" { 100 } else { 0 };
    let mut cook = new_ambition("cook", "random", &now(&fresh, 0, &nothing_held), 3, &[], ZERO).expect("cook");
    for _ in 0..5 {
        refresh(&mut cook, &now(&fresh, 0, &stocked), 3, &bid("shrimp", Some(5.0)));
    }
    assert_eq!(demand_stones(&cook), 0, "100 shrimp held: no demand quota");
    refresh(&mut cook, &now(&fresh, 0, &nothing_held), 3, &bid("shrimp", Some(5.0)));
    assert_eq!(demand_stones(&cook), 1, "no stock: the bid motivates a quota");
    for stone in &mut cook.milestones {
        if stone.origin.starts_with("demand:") {
            stone.status = MilestoneStatus::Done;
        }
    }
    for _ in 0..5 {
        refresh(&mut cook, &now(&fresh, 0, &stocked), 3, &bid("shrimp", Some(5.0)));
    }
    assert_eq!(demand_stones(&cook), 1, "the quota's own stock never reinserts it");
}

/// Finding 1 (growth): done milestones stay bounded however long a bot
/// serves the market.
#[test]
fn done_milestones_stay_bounded() {
    let mut cook = new_ambition("cook", "random", &now(&fresh, 0, &nothing_held), 3, &[], ZERO).expect("cook");
    for _ in 0..(MAX_DONE_KEPT * 3) {
        refresh(&mut cook, &now(&fresh, 0, &nothing_held), 3, &bid("shrimp", Some(5.0)));
        for stone in &mut cook.milestones {
            if stone.origin.starts_with("demand:") {
                stone.status = MilestoneStatus::Done;
            }
        }
    }
    refresh(&mut cook, &now(&fresh, 0, &nothing_held), 3, &bid("shrimp", Some(5.0)));
    let done = cook.milestones.iter().filter(|stone| stone.status == MilestoneStatus::Done).count();
    assert!(done <= MAX_DONE_KEPT, "{done} done milestones kept");
    let mut ids: Vec<&str> = cook.milestones.iter().map(|stone| stone.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), cook.milestones.len(), "ids stay unique");
}

/// Finding 5: a bid below the input cost (steel platebody at 1 coin) never
/// makes a smith forge at a loss; an unpriced chain neither.
#[test]
fn unprofitable_bids_never_make_a_quota() {
    let smithing = |skill: &str| if skill == "smithing" { 9 } else { 1 };
    let mut smith = new_ambition("smith", "random", &now(&smithing, 0, &nothing_held), 5, &[], ZERO).expect("smith");
    refresh(&mut smith, &now(&smithing, 0, &nothing_held), 5, &bid("bronze_platebody", Some(-4.0)));
    assert_eq!(demand_stones(&smith), 0, "negative margin");
    refresh(&mut smith, &now(&smithing, 0, &nothing_held), 5, &bid("bronze_platebody", None));
    assert_eq!(demand_stones(&smith), 0, "no margin known");
    refresh(&mut smith, &now(&smithing, 0, &nothing_held), 5, &bid("bronze_platebody", Some(12.0)));
    assert_eq!(demand_stones(&smith), 1, "a paying bid motivates production");
}

/// Finding 3: a draw that gives the bot no milestone is refused (typed),
/// never an ambition achieved at birth and redrawn every pass.
#[test]
fn a_draw_with_no_milestone_is_refused() {
    let master = |_: &str| 99u8;
    let held = nothing_held;
    assert_eq!(
        new_ambition("crafter", "random", &now(&master, 0, &held), 1, &[], ZERO).err(),
        Some(AmbitionError::NoMilestone("crafter".to_string()))
    );
    assert_eq!(new_ambition("bogus", "random", &now(&master, 0, &held), 1, &[], ZERO).err(), Some(AmbitionError::UnknownKind("bogus".to_string())));
    assert!(new_ambition(GENERALIST, "operator", &now(&master, 0, &held), 1, &[], ZERO).is_ok(), "the generalist holds no milestone by design");
    let extended = new_ambition_extended("crafter", "coverage:crafting", &now(&master, 0, &held), 1, &[], ZERO).expect("extended");
    assert!(open_count(&extended) > 0 && !achieved(&extended));
    assert!(extended.milestones.iter().all(|stone| stone.origin == "extend"), "{:?}", extended.milestones);
}

/// Finding 7: a rich bot's merchant ambition starts past the coins it
/// already holds (no useless 5 000-coin GetRich first).
#[test]
fn a_rich_merchant_starts_past_its_coins() {
    let merchant = new_ambition(MERCHANT, "coverage:merchant", &now(&fresh, 30_000, &nothing_held), 1, &[], ZERO).expect("merchant");
    let first = merchant.milestones.iter().find(|stone| stone.status != MilestoneStatus::Done).expect("open");
    assert_eq!(first.target, Target::Coins { amount: 50_000 });
}

/// Finding 3: the sole merchant past 1M (or any sole holder at the end
/// of its ladder) extends its ladder instead of leaving the role.
#[test]
fn an_exhausted_ladder_extends() {
    let rich = now(&fresh, 2_000_000, &nothing_held);
    assert_eq!(new_ambition(MERCHANT, "coverage:merchant", &rich, 1, &[], ZERO).err(), Some(AmbitionError::NoMilestone(MERCHANT.to_string())));
    let merchant = new_ambition_extended(MERCHANT, "coverage:merchant", &rich, 1, &[], ZERO).expect("extended");
    assert!(merchant.milestones.iter().any(|stone| stone.target == Target::Coins { amount: 4_000_000 }), "{:?}", merchant.milestones);
    let mut miner = new_ambition("miner", "random", &now(&fresh, 0, &nothing_held), 3, &[], ZERO).expect("miner");
    let maxed = |_: &str| 99u8;
    for _ in 0..20 {
        for stone in &mut miner.milestones {
            stone.status = MilestoneStatus::Done;
        }
        refresh(&mut miner, &now(&maxed, 0, &nothing_held), 3, &[]);
    }
    for stone in &mut miner.milestones {
        stone.status = MilestoneStatus::Done;
    }
    assert!(achieved(&miner));
    assert!(extend(&mut miner, &now(&maxed, 0, &nothing_held), 3) > 0, "a produce rung repeats at 99");
    assert!(!achieved(&miner));
    let warrior = new_ambition_extended("warrior", "coverage:melee", &now(&maxed, 0, &nothing_held), 3, &[], ZERO).expect("warrior");
    assert!(warrior.milestones.iter().any(|stone| matches!(stone.target, Target::Equip { .. })), "{:?}", warrior.milestones);
    let mid = |skill: &str| if skill == "attack" || skill == "strength" || skill == "defence" { 70u8 } else { 1 };
    let mut climber = new_ambition_extended("warrior", "coverage:melee", &now(&mid, 0, &nothing_held), 3, &[], ZERO).expect("warrior 70");
    assert!(climber.milestones.iter().any(|stone| matches!(&stone.target, Target::Level { level: 75, .. })), "{:?}", climber.milestones);
    for stone in &mut climber.milestones {
        stone.status = MilestoneStatus::Done;
    }
    assert!(extend(&mut climber, &now(&mid, 0, &nothing_held), 3) > 0);
}

/// Finding 4: refresh names the level and coin milestones it closes (the
/// brain then weighs an optional change, as for a closed goal).
#[test]
fn refresh_reports_the_milestones_it_closes() {
    let mut smith = new_ambition("smith", "random", &now(&fresh, 0, &nothing_held), 3, &[], ZERO).expect("smith");
    let mining = |skill: &str| if skill == "mining" { 12 } else { 1 };
    let closed = refresh(&mut smith, &now(&mining, 0, &nothing_held), 3, &[]);
    assert_eq!(closed, vec!["m1".to_string()], "mining 10 met");
    assert!(refresh(&mut smith, &now(&mining, 0, &nothing_held), 3, &[]).is_empty(), "closed once");
}

fn fleet_bot(seed: u64) -> FleetBot {
    FleetBot {
        account: "b0".to_string(),
        levels: Vec::new(),
        ironman: false,
        merchant_archetype: false,
        market_affinity: 0.5,
        seed,
        kind: Some("fisher".to_string()),
    }
}

/// Finding 4: an open gap is filled by a redundant bot past the 72 000
/// tick throttle (the user's rule), never by the sole holder of its own
/// group, never by a throttled bot; no gap, no change.
#[test]
fn coverage_repair_fills_gaps_within_the_throttle() {
    let fisher = new_ambition("fisher", "random", &now(&fresh, 0, &nothing_held), 7, &[], ZERO).expect("fisher");
    let gaps = vec!["smithing".to_string()];
    let late = |_: i64| AMBITION_CHANGE_TICKS;
    let early = |_: i64| AMBITION_CHANGE_TICKS - 1;
    let filled = coverage_repair(&fisher, &fleet_bot(7), &gaps, true, &[], &late).expect("fills the gap");
    assert_eq!((filled.0.as_str(), filled.1.as_str()), ("smith", "coverage:smithing"));
    assert_eq!(coverage_repair(&fisher, &fleet_bot(7), &gaps, false, &[], &late), None, "sole holder keeps its group");
    assert_eq!(coverage_repair(&fisher, &fleet_bot(7), &gaps, true, &[], &early), None, "72 000 ticks between changes");
    assert_eq!(coverage_repair(&fisher, &fleet_bot(7), &[], true, &[], &late), None, "no gap");
}

/// Finding 7: the revision stamp is `at_secs`; old files wrote `at`.
#[test]
fn revision_seconds_read_the_old_key() {
    let mut ambition = new_ambition("fisher", "random", &now(&fresh, 0, &nothing_held), 7, &[], ZERO).expect("fisher");
    ambition.revisions.push(super::Revision { tick: 9, at_secs: 5_000, from: "smith".to_string(), to: "fisher".to_string(), why: "x".to_string() });
    let value = ambition_to_value(&ambition);
    let back = ambition_from_value(&value).expect("parses");
    assert_eq!(back, ambition);
    let text = lifescape_json::stringify(&value).replace("\"at_secs\"", "\"at\"");
    let legacy = ambition_from_value(&lifescape_json::parse(&text).expect("json")).expect("old file");
    assert_eq!(legacy.revisions[0].at_secs, 5_000, "old key read");
}
