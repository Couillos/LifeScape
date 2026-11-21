//! Protected accounts (user 2026-11-21): Zezima, N0valyfe and Yogosun
//! always hold the overall ambition, whatever the draw, the fleet pass or
//! a revision.

use crate::ambition::{new_ambition, revise, BotNow, Stamp, OVERALL};
use crate::ambition_fleet::{assign_fleet, draw_kind, neediest_kind, FleetBot, PROTECTED_WHY};
use crate::ambition_review::{mark_rule, OPERATOR_RULE};
use crate::model::{Archetype, GoalKind, Life, Profile};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn bot(account: &str) -> FleetBot {
    FleetBot {
        account: account.to_string(),
        levels: vec![("woodcutting".to_string(), 70)],
        ironman: false,
        market_affinity: 0.5,
        seed: 3,
        kind: None,
        market_maker: false,
        operator: false,
    }
}

fn life(account: &str, kind: &str) -> Life {
    let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: Some(new_ambition(kind, "random", &now, 7, &[], ZERO).expect("ambition")),
        goal_origin: None,
        memory: Default::default(),
    }
}

#[test]
fn a_protected_account_is_always_drawn_overall() {
    for name in ["Zezima", "N0valyfe", "Yogosun", "zezima", "YOGOSUN"] {
        assert_eq!(draw_kind(&bot(name), &[], &[], &[]), (OVERALL.to_string(), PROTECTED_WHY.to_string()), "{name}");
        assert_eq!(draw_kind(&bot(name), &["coverage:woodcutting".to_string()], &[], &["overall"]).0, OVERALL, "{name}: gaps and exclusions do not matter");
        assert_eq!(neediest_kind(&bot(name), &[], &[]).map(|row| row.0), Some(OVERALL.to_string()), "{name}");
    }
    assert_ne!(draw_kind(&bot("jane_doe"), &[], &[], &[]).0, OVERALL, "an ordinary bot never draws overall");
}

#[test]
fn the_fleet_pass_gives_overall_to_a_protected_account_and_no_gap() {
    let bots = vec![bot("Zezima"), bot("jane_doe")];
    let given = assign_fleet(&bots, &[]);
    let zezima = given.iter().find(|row| row.account == "Zezima").expect("Zezima assigned");
    assert_eq!((zezima.kind.as_str(), zezima.why.as_str()), (OVERALL, PROTECTED_WHY));
    assert!(given.iter().any(|row| row.account == "jane_doe"), "the other bot is assigned too");
}

#[test]
fn an_overall_ambition_of_a_protected_account_is_never_replaced() {
    let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    let woodcutter = new_ambition("woodcutter", "random", &now, 7, &[], ZERO).expect("woodcutter");
    let mut zezima = life("N0valyfe", OVERALL);
    revise(&mut zezima, woodcutter.clone(), "achieved", Stamp { tick: 5, secs: 5 });
    let held = zezima.ambition.as_ref().expect("ambition");
    assert_eq!((held.kind.as_str(), held.revisions.len()), (OVERALL, 0), "refused, no revision");
    let mut other = life("jane_doe", OVERALL);
    revise(&mut other, woodcutter, "achieved", Stamp { tick: 5, secs: 5 });
    assert_eq!(other.ambition.as_ref().map(|held| held.kind.as_str()), Some("woodcutter"), "an ordinary account is revised");
    // Giving overall to a protected account holding something else works.
    let mut old = life("Yogosun", "woodcutter");
    let overall = new_ambition(OVERALL, PROTECTED_WHY, &now, 7, &[], ZERO).expect("overall");
    revise(&mut old, overall, PROTECTED_WHY, Stamp { tick: 6, secs: 6 });
    assert_eq!(old.ambition.as_ref().map(|held| held.kind.as_str()), Some(OVERALL));
}

#[test]
fn overall_is_marked_operator() {
    let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    let mut overall = new_ambition(OVERALL, PROTECTED_WHY, &now, 7, &[], ZERO).expect("overall");
    mark_rule(&mut overall, &bot("Zezima"));
    assert_eq!(overall.draw_rule, OPERATOR_RULE);
}
