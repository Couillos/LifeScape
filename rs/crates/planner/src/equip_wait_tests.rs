use super::unmade_equip_piece;
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn equip_life(style: &str, pieces: &[(&str, NodeStatus)]) -> Life {
    let mut life = Life {
        schema: 1,
        account: "lily_north".to_string(),
        created_at: 0,
        seed: 0,
        profile: Profile::new(false, 0.5, 0.5, 0, Archetype::Producer),
        goal: GoalKind::EquipBest { style: style.to_string() },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    };
    let mut deps = Vec::new();
    for (index, (item, status)) in pieces.iter().enumerate() {
        let id = format!("n{index}");
        let mut obtain = Node::new(&id, NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
        obtain.status = status.clone();
        deps.push(id);
        life.nodes.push(obtain);
    }
    life.nodes.push(Node::new("n99", NodeKind::EquipBest { style: style.to_string() }, deps, 1));
    life
}

/// Live 2026-11-21: the amulet of power nobody makes, not held, no ask.
#[test]
fn an_unheld_piece_nobody_makes_waits() {
    let life = equip_life("magic", &[("magic_staff", NodeStatus::Done), ("amulet_of_power", NodeStatus::Abandoned)]);
    let nothing_held = |_: &str| false;
    let made = |item: &str| item != "amulet_of_power";
    assert_eq!(unmade_equip_piece(&life, &nothing_held, &made), Some("amulet_of_power".to_string()));
}

/// Held, or obtainable now (market, shop, a fleet smith): a real failure.
#[test]
fn a_held_or_obtainable_piece_is_a_real_failure() {
    let life = equip_life("magic", &[("amulet_of_power", NodeStatus::Abandoned)]);
    assert_eq!(unmade_equip_piece(&life, &|item: &str| item == "amulet_of_power", &|_: &str| false), None, "held");
    assert_eq!(unmade_equip_piece(&life, &|_: &str| false, &|_: &str| true), None, "obtainable now");
}

/// Only the equip goal waits; another goal's graph never does.
#[test]
fn only_an_equip_goal_waits() {
    let mut life = equip_life("magic", &[("amulet_of_power", NodeStatus::Abandoned)]);
    life.goal = GoalKind::Produce { item: "amulet_of_power".to_string(), count: 1 };
    assert_eq!(unmade_equip_piece(&life, &|_: &str| false, &|_: &str| false), None);
}
