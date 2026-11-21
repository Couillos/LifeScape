//! Operator pins (`create-user --goal ambition:<skill>:<level>`, user
//! decision 2026-11-14): the right kind per skill, never redrawn nor
//! changed, a thrice-blocked milestone dropped instead, extension toward
//! the level, the pin lifted once reached, kept across the life file.

use super::{
    ambition_from_value, ambition_to_value, achieved, extend, on_goal_closed, pinned, pinned_ambition, release_pin, AmbitionError, BotNow, CloseOutcome,
    MilestoneStatus, Stamp, Target, MILESTONE_MAX_BLOCKS,
};
use crate::ambition_fleet::FleetBot;
use crate::model::{Archetype, GoalKind, Life, Profile};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn fresh(skill: &str) -> u8 {
    if skill == "hitpoints" {
        10
    } else {
        1
    }
}

fn nothing_held(_: &str) -> i64 {
    0
}

fn fresh_now() -> BotNow<'static> {
    BotNow { level_of: &fresh, coins: 0, held: &nothing_held }
}

fn fleet_bot() -> FleetBot {
    FleetBot { account: "pin".to_string(), levels: Vec::new(), ironman: false, market_affinity: 0.5, seed: 3, kind: None, market_maker: false }
}

fn life_pinned(skill: &str) -> Life {
    Life {
        schema: 1,
        account: "pin".to_string(),
        created_at: 0,
        seed: 3,
        profile: Profile::new(false, 0.5, 0.5, 3, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: Some(pinned_ambition(skill, 99, &fresh_now(), 3, &[], ZERO).expect("pinnable")),
        goal_origin: None,
        memory: Default::default(),
    }
}

/// The seven skills the operator pins map to their catalogue kind, each
/// with an open milestone at birth and the pin recorded.
#[test]
fn each_pinned_skill_gets_its_kind() {
    let expected = [
        ("magic", "mage"),
        ("firemaking", "firemaker"),
        ("mining", "miner"),
        ("smithing", "smith"),
        ("ranged", "ranger"),
        ("crafting", "crafter"),
        ("cooking", "cook"),
    ];
    for (skill, kind) in expected {
        let ambition = pinned_ambition(skill, 99, &fresh_now(), 3, &[], ZERO).expect("pinnable");
        assert_eq!(ambition.kind, kind, "{skill}");
        assert!(pinned(&ambition));
        assert_eq!(ambition.pin.as_ref().map(|pin| (pin.skill.as_str(), pin.level)), Some((skill, 99)));
        assert!(ambition.why.starts_with("operator:pin:"), "{}", ambition.why);
        assert!(ambition.milestones.iter().any(|stone| stone.status != MilestoneStatus::Done), "{skill}: an open milestone");
    }
}

/// No kind climbs hitpoints (melee extension never trains it), unknown
/// skills and out-of-range levels are refused, typed.
#[test]
fn unclimbable_pins_are_refused() {
    for (skill, level) in [("hitpoints", 99), ("fletching", 99), ("magic", 1), ("magic", 100)] {
        let refused = pinned_ambition(skill, level, &fresh_now(), 3, &[], ZERO);
        assert_eq!(refused.err(), Some(AmbitionError::NotPinnable(skill.to_string())), "{skill} {level}");
    }
}

/// A pinned ambition is never changed (optional change, coverage repair),
/// even redundant, past the throttle, with a gap open.
#[test]
fn a_pin_blocks_every_change() {
    let life = life_pinned("smithing");
    let ambition = life.ambition.as_ref().expect("ambition");
    assert!(pinned(ambition), "held by the pin: kept past the 72 000-tick clock");
    let long_ago = |_: i64| u64::MAX;
    assert!(!crate::ambition::may_change(&ambition_free(ambition), &|_| 0), "the clock runs from the draw");
    assert!(crate::ambition::may_change(&ambition_free(ambition), &long_ago));
}

/// Blocked `MILESTONE_MAX_BLOCKS` times: the milestone is dropped and the
/// pinned ambition kept (an unpinned one would be redrawn).
#[test]
fn a_thrice_blocked_milestone_is_dropped_not_redrawn() {
    let mut life = life_pinned("magic");
    let first = life.ambition.as_ref().and_then(|ambition| ambition.milestones.first()).map(|stone| stone.id.clone()).expect("m1");
    for round in 1..=MILESTONE_MAX_BLOCKS {
        life.goal_origin = Some(first.clone());
        let outcome = on_goal_closed(&mut life, true);
        if round < MILESTONE_MAX_BLOCKS {
            assert_eq!(outcome, CloseOutcome::Blocked { id: first.clone(), count: round });
        } else {
            assert_eq!(outcome, CloseOutcome::Dropped { id: first.clone(), count: round });
        }
    }
    let ambition = life.ambition.as_ref().expect("kept");
    assert_eq!(ambition.kind, "mage");
    assert!(pinned(ambition));
    assert!(ambition.milestones.iter().all(|stone| stone.id != first), "dropped");
}

/// Past the ladder, the extension climbs the pinned skill toward 99.
#[test]
fn the_extension_climbs_the_pinned_skill() {
    for skill in ["magic", "firemaking", "mining", "smithing", "ranged", "crafting", "cooking"] {
        let mut life = life_pinned(skill);
        let ambition = life.ambition.as_mut().expect("ambition");
        let high = |name: &str| if name == skill { 90 } else { 60 };
        let now = BotNow { level_of: &high, coins: 0, held: &nothing_held };
        for stone in &mut ambition.milestones {
            stone.status = MilestoneStatus::Done;
        }
        ambition.rung = crate::ambition_catalog::spec(&ambition.kind).map(|found| found.ladder.len()).unwrap_or(0);
        assert!(achieved(ambition), "{skill}: ladder done");
        extend(ambition, &now, 3);
        let climbs = ambition.milestones.iter().any(|stone| {
            stone.status != MilestoneStatus::Done && matches!(&stone.target, Target::Level { skill: trained, level } if trained == skill && *level > 90)
        });
        assert!(climbs, "{skill}: {:?}", ambition.milestones.iter().map(|stone| &stone.label).collect::<Vec<_>>());
    }
}

/// The pin lifts once its level is reached, never before; it survives
/// the life file.
#[test]
fn the_pin_lifts_at_its_level_and_survives_the_file() {
    let mut ambition = pinned_ambition("crafting", 99, &fresh_now(), 3, &[], ZERO).expect("pinnable");
    let reloaded = ambition_from_value(&ambition_to_value(&ambition)).expect("reads back");
    assert_eq!(reloaded, ambition);
    assert_eq!(release_pin(&mut ambition, &|_| 98), None);
    assert!(pinned(&ambition));
    let lifted = release_pin(&mut ambition, &|_| 99).expect("lifted");
    assert_eq!((lifted.skill.as_str(), lifted.level), ("crafting", 99));
    assert!(!pinned(&ambition));
    let unpinned = ambition_from_value(&ambition_to_value(&ambition)).expect("reads back");
    assert_eq!(unpinned.pin, None);
}

/// The overall highscore ambition (user 2026-11-15): operator-only, held
/// for good (never redrawn, changed, repaired, nor "achieved"), never
/// drawn by the fleet, and its goal is always the lowest F2P skill.
#[test]
fn the_overall_ambition_climbs_the_lowest_skill_for_good() {
    let ambition = super::new_ambition(super::OVERALL, "operator:overall", &fresh_now(), 3, &[], ZERO).expect("ladderless kind");
    assert!(pinned(&ambition), "held like a pin");
    assert!(!achieved(&ambition), "an empty ladder is never achieved");
    for seed in 0..50 {
        let mut bot = fleet_bot();
        bot.seed = seed;
        let (kind, _) = crate::ambition_fleet::draw_kind(&bot, &[], &[], &[]);
        assert_ne!(kind, super::OVERALL, "never a fleet draw (seed {seed})");
    }
    let mut life = life_pinned("magic");
    life.ambition = Some(ambition);
    // Every skill 30 but runecraft 7: runecraft is the cheapest level.
    let level_of = |skill: &str| if skill == "runecraft" { 7 } else { 30 };
    let (goal, origin) = crate::goals::draw_goal_origin(&life, &level_of, &|_| 0).expect("a goal");
    assert_eq!(goal, GoalKind::MaxSkills { skills: vec!["runecraft".to_string()], level: 15 });
    assert_eq!(origin, None);
}

fn ambition_free(ambition: &crate::ambition::Ambition) -> crate::ambition::Ambition {
    let mut free = ambition.clone();
    free.pin = None;
    free
}
