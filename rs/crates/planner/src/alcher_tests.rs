//! The alchemist: operator switch, pinned, ladderless, money fallback.

use super::{become_alcher, is_alcher};
use crate::ambition::{ladderless, new_ambition, pinned, BotNow, Stamp, ALCHER};
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn mage_life() -> Life {
    let now = BotNow { level_of: &|_| 60, coins: 0, held: &|_| 0 };
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "steel_plateskirt".to_string(), count: 28 }, Vec::new(), 28);
    node.status = NodeStatus::Ready;
    Life {
        schema: 1,
        account: "ryan_mason".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: vec![node],
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: new_ambition("mage", "random", &now, 7, &[], ZERO).ok(),
        goal_origin: None,
        memory: Default::default(),
    }
}

/// ryan_mason 2026-11-18: the operator makes him the alchemist; his
/// running production is abandoned, the ambition is held for good, the
/// fleet never draws it, and with nothing to alch he earns money.
#[test]
fn the_operator_makes_an_alchemist() {
    let mut life = mage_life();
    assert!(!is_alcher(&life));
    let now = BotNow { level_of: &|_| 60, coins: 0, held: &|_| 0 };
    let abandoned = become_alcher(&mut life, &now, ZERO).expect("alcher");
    assert_eq!(abandoned, vec!["n1".to_string()], "the running production goes");
    assert!(is_alcher(&life));
    let ambition = life.ambition.as_ref().expect("ambition");
    assert!(pinned(ambition) && ladderless(ALCHER), "operator-only, held for good");
    let drawn = crate::goals::draw_goal_origin(&life, &|_| 60, &|_| 0).expect("a goal");
    assert_eq!(drawn, (GoalKind::MakeMoney, None), "nothing to alch: money funds the buys");
}
