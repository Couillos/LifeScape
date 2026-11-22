//! One session at a time (user decision 2026-11-22): a climb buys what one
//! 6000-tick session of its method burns, a partial tranche is worked.

use super::resize_climb_inputs;
use crate::model::{Alternative, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};
use crate::replan::{need_count_from, need_count_with};
use crate::work_rate::WorkRate;

const METHOD: &str = "smith:steel_med_helm";

/// A smith whose session is 500 actions, one steel bar each, 40 XP a
/// bar: the whole 40 -> 70 climb burns far more than 500 bars.
fn session_rate(_method: &str, _level: u8) -> Option<WorkRate> {
    Some(WorkRate {
        actions_6000: 500.0,
        xp_6000: vec![("smithing".to_string(), 500.0 * 40.0)],
        flows_6000: vec![("steel_bar".to_string(), "input".to_string(), -500.0)],
    })
}

fn climb() -> Node {
    let mut node = Node::new("n7", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 70 }, Vec::new(), 70);
    node.method = Some(METHOD.to_string());
    node
}

#[test]
fn a_climb_buys_one_session_of_inputs_not_the_whole_tranche() {
    let node = climb();
    let tranche = crate::sizing::tranche_need_with(&session_rate, METHOD, 40, 70, "steel_bar").expect("tranche");
    assert!(tranche > 5000, "the whole climb is huge: {tranche}");
    assert_eq!(crate::sizing::session_need_with(&session_rate, METHOD, 40, "steel_bar"), Some(500));
    assert_eq!(need_count_with(&node, "steel_bar", Some(40), &session_rate), 500, "one session");
    // A climb shorter than a session keeps its (smaller) remaining need.
    let mut short = climb();
    short.kind = NodeKind::ReachLevel { skill: "smithing".to_string(), level: 41 };
    let remaining = crate::sizing::tranche_need_with(&session_rate, METHOD, 40, 41, "steel_bar").expect("tranche");
    assert!(remaining < 500);
    assert_eq!(need_count_with(&short, "steel_bar", Some(40), &session_rate), remaining);
    // Without a measure the prior still caps it at its own session.
    let prior = need_count_from(&node, "steel_bar", Some(40));
    let session = crate::sizing::session_need_with(&crate::work_rate::prior_rate, METHOD, 40, "steel_bar").expect("prior session");
    assert!(prior <= session, "{prior} <= {session}");
}

#[test]
fn a_random_recipe_session_carries_the_margin() {
    let rate = |_: &str, _: u8| {
        Some(WorkRate {
            actions_6000: 400.0,
            xp_6000: vec![("smithing".to_string(), 400.0 * 12.5)],
            flows_6000: vec![("iron_ore".to_string(), "input".to_string(), -400.0)],
        })
    };
    let session = crate::sizing::session_need_with(&rate, "smelt:iron_bar", 15, "iron_ore").expect("session");
    assert!(session >= 440, "400 expected ores plus the 10 % margin: {session}");
}

fn life_with(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "ryan_mason".to_string(),
        created_at: 0,
        seed: 0,
        profile: Profile::new(false, 0.5, 0.5, 0, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// Existing state: an open obtain of 12 447 bars feeding a climb shrinks
/// to one session; a second pass changes nothing; an obtain for another
/// purpose (no climb above it) keeps its size.
#[test]
fn a_persisted_giant_obtain_shrinks_to_one_session() {
    let mut giant = Node::new("n5", NodeKind::ObtainItems { item: "steel_bar".to_string(), count: 12447 }, Vec::new(), 12447);
    giant.status = NodeStatus::Ready;
    giant.method = Some("market:buy".to_string());
    let mut other = Node::new("n6", NodeKind::ObtainItems { item: "steel_bar".to_string(), count: 9000 }, Vec::new(), 9000);
    other.status = NodeStatus::Ready;
    let mut climb = climb();
    climb.deps = vec!["n5".to_string()];
    climb.status = NodeStatus::Pending;
    climb.alternatives.push(Alternative { method: METHOD.to_string(), est_ticks: 0, est_coins: 0 });
    let mut life = life_with(vec![giant, other, climb]);
    let lines = resize_climb_inputs(&mut life, &|_| 40, &session_rate);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(matches!(&life.nodes[0].kind, NodeKind::ObtainItems { count: 500, .. }));
    assert_eq!(life.nodes[0].progress_target, 500);
    assert!(matches!(&life.nodes[1].kind, NodeKind::ObtainItems { count: 9000, .. }), "a fixed quantity keeps its size");
    assert!(resize_climb_inputs(&mut life, &|_| 40, &session_rate).is_empty(), "idempotent");
}
