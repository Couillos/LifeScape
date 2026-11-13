//! Tutorial gate: on Tutorial Island the tutorial is the only task that
//! may run. The gate decides on live data only (never on a missing
//! snapshot), closes the tutorial node on positive proof only (its own
//! report, or the bot seen off the island), and holds every launch when
//! the tutorial was abandoned while the bot is still on the island
//! (the operator decides, rule 7).

#![forbid(unsafe_code)]

use lifescape_json::Value;
use lifescape_planner::{complete_node, insert_ready, next_id, Alternative, Life, Node, NodeKind, NodeStatus};

/// Island rectangles (surface and the two underground areas). The zone
/// labels are wrong on parts of the island ("Wizards' Tower" at
/// (3072,3133), "Bedabin Camp"...), so labels never decide alone.
const ISLAND_SURFACE: ((i32, i32), (i32, i32)) = ((3050, 3160), (3050, 3140));
const ISLAND_MINE: ((i32, i32), (i32, i32)) = ((3070, 3120), (9495, 9530));

fn inside(rect: ((i32, i32), (i32, i32)), x: i32, z: i32) -> bool {
    (rect.0 .0..=rect.0 .1).contains(&x) && (rect.1 .0..=rect.1 .1).contains(&z)
}

/// Where the bot is, from the live snapshot: `Some(true)` on the island,
/// `Some(false)` on the mainland, `None` without a position (no snapshot
/// yet, light heartbeat only): the caller must not decide then.
pub fn on_tutorial_island(live: &Value) -> Option<bool> {
    let pos = live.get("position").as_arr()?;
    if pos.len() != 3 {
        return None;
    }
    let x = pos[1].as_i64()? as i32;
    let z = pos[2].as_i64()? as i32;
    if x <= 0 && z <= 0 {
        return None;
    }
    if inside(ISLAND_SURFACE, x, z) || inside(ISLAND_MINE, x, z) {
        return Some(true);
    }
    if live.get("zone").as_str() == Some("Tutorial Island") {
        return Some(true);
    }
    Some(false)
}

fn is_tutorial(node: &Node) -> bool {
    matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "tutorial")
}

/// The open tutorial node, created once (never duplicated).
pub fn ensure_tutorial_node(life: &mut Life) -> usize {
    if let Some(at) = life.nodes.iter().position(|node| is_tutorial(node) && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)) {
        if life.nodes[at].method.as_deref().is_none_or(str::is_empty) {
            life.nodes[at].method = Some("tutorial".to_string());
        }
        return at;
    }
    let mut node = Node::new(&next_id(life), NodeKind::CompleteQuest { quest: "tutorial".to_string() }, Vec::new(), 1);
    node.method = Some("tutorial".to_string());
    node.alternatives.push(Alternative { method: "tutorial".to_string(), est_ticks: 6000, est_coins: 0 });
    insert_ready(life, node)
}

/// What the launch pass may do this pass.
#[derive(Debug, PartialEq)]
pub enum TutorialGate {
    /// Launch the tutorial node at this index (and nothing else).
    Launch(usize),
    /// Launch nothing at all this pass (reason for the log).
    Hold(String),
    /// The open tutorial node was closed: the bot was seen off the
    /// island with no tutorial running.
    Closed(String),
    /// No tutorial concern: the normal graph runs.
    Clear,
}

/// Gate for one launch pass (graph already normalized, no node active).
pub fn tutorial_gate(life: &mut Life, live: &Value) -> TutorialGate {
    let Some(on_island) = on_tutorial_island(live) else {
        return TutorialGate::Hold("no live position yet".to_string());
    };
    let open = life.nodes.iter().position(|node| is_tutorial(node) && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned | NodeStatus::Active));
    if on_island {
        if let Some(at) = open {
            return TutorialGate::Launch(at);
        }
        let abandoned = life.nodes.iter().find(|node| is_tutorial(node) && node.status == NodeStatus::Abandoned);
        let done = life.nodes.iter().any(|node| is_tutorial(node) && node.status == NodeStatus::Done);
        if let (Some(node), false) = (abandoned, done) {
            return TutorialGate::Hold(format!("tutorial {} abandoned on the island ({}), operator decides", node.id, node.last_error));
        }
        return TutorialGate::Launch(ensure_tutorial_node(life));
    }
    if let Some(at) = open {
        let target = life.nodes[at].progress_target;
        complete_node(life, at, target);
        return TutorialGate::Closed(life.nodes[at].id.clone());
    }
    TutorialGate::Clear
}

#[cfg(test)]
mod tests {
    use super::{on_tutorial_island, tutorial_gate, TutorialGate};
    use lifescape_json::{object, str_value, Value};
    use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

    fn at(x: i64, z: i64) -> Value {
        object(vec![("position", Value::Arr(vec![Value::Int(0), Value::Int(x), Value::Int(z)]))])
    }

    fn fresh_life() -> Life {
        let mut gather = Node::new("n1", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 5 }, Vec::new(), 5);
        gather.status = NodeStatus::Ready;
        Life {
            schema: 1,
            account: "b0".to_string(),
            created_at: 0,
            seed: 1,
            profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
            goal: GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 },
            nodes: vec![gather],
            engagement: None,
            active: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn island_reads_rectangles_before_labels() {
        assert_eq!(on_tutorial_island(&at(3104, 3093)), Some(true), "surface");
        assert_eq!(on_tutorial_island(&at(3072, 3133)), Some(true), "labelled Wizards' Tower, still the island");
        assert_eq!(on_tutorial_island(&at(3107, 9512)), Some(true), "rat pit");
        assert_eq!(on_tutorial_island(&at(3079, 9505)), Some(true), "mine");
        assert_eq!(on_tutorial_island(&at(3222, 3218)), Some(false), "Lumbridge");
        assert_eq!(on_tutorial_island(&at(3090, 3264)), Some(false), "Draynor");
        assert_eq!(on_tutorial_island(&Value::Null), None, "no snapshot: undecided");
        assert_eq!(on_tutorial_island(&object(vec![("zone", str_value("Tutorial Island"))])), None, "a label alone is not a position");
        assert_eq!(on_tutorial_island(&at(0, 0)), None, "pre-login zero position");
    }

    /// bt7ktb8c / Ironz5srdb5s: an empty snapshot launched fishing on the
    /// island. Now nothing launches until the position is known.
    #[test]
    fn missing_live_holds_every_launch() {
        let mut life = fresh_life();
        assert!(matches!(tutorial_gate(&mut life, &Value::Null), TutorialGate::Hold(_)));
        assert_eq!(life.nodes.len(), 1, "no node invented");
        assert_eq!(life.nodes[0].status, NodeStatus::Ready, "nothing closed, nothing launched");
    }

    #[test]
    fn island_launches_only_the_tutorial_then_mainland_closes_it() {
        let mut life = fresh_life();
        let launched = match tutorial_gate(&mut life, &at(3104, 3093)) {
            TutorialGate::Launch(index) => index,
            other => panic!("island launches the tutorial, got {other:?}"),
        };
        assert!(matches!(&life.nodes[launched].kind, NodeKind::CompleteQuest { quest } if quest == "tutorial"));
        assert_eq!(tutorial_gate(&mut life, &at(3104, 3093)), TutorialGate::Launch(launched), "reused, never duplicated");
        assert_eq!(tutorial_gate(&mut life, &at(3222, 3218)), TutorialGate::Closed(life.nodes[launched].id.clone()));
        assert_eq!(life.nodes[launched].status, NodeStatus::Done);
        assert_eq!(tutorial_gate(&mut life, &at(3222, 3218)), TutorialGate::Clear);
    }

    /// A running tutorial is never closed by position (the report closes
    /// it), and an abandoned tutorial on the island holds everything.
    #[test]
    fn running_tutorial_waits_and_abandoned_one_holds() {
        let mut life = fresh_life();
        let index = match tutorial_gate(&mut life, &at(3104, 3093)) {
            TutorialGate::Launch(index) => index,
            other => panic!("launch, got {other:?}"),
        };
        life.nodes[index].status = NodeStatus::Active;
        assert_eq!(tutorial_gate(&mut life, &at(3222, 3218)), TutorialGate::Clear, "running node untouched");
        assert_eq!(life.nodes[index].status, NodeStatus::Active);
        life.nodes[index].status = NodeStatus::Abandoned;
        life.nodes[index].last_error = "Failed: combat instructor not found".to_string();
        assert!(matches!(tutorial_gate(&mut life, &at(3104, 3093)), TutorialGate::Hold(reason) if reason.contains("operator")));
        assert_eq!(life.nodes[0].status, NodeStatus::Ready, "the gather never launches on the island");
    }
}
