//! Market waits never idle the worker (user rule 2026-11-13).

use super::parked_waits;
use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn obtain(id: &str, method: Option<&str>, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ObtainItems { item: "amulet_of_power".to_string(), count: 1 }, Vec::new(), 1);
    node.method = method.map(str::to_string);
    node.status = status;
    node
}

fn life_of(nodes: Vec<Node>, history: Vec<HistoryEntry>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes,
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// A parked wait is a ready `market:buy` obtain whose own last history
/// entry is `awaiting_fill`; a later entry (filled, failed, partial) or
/// another method ends it.
#[test]
fn parked_waits_read_the_node_history() {
    let life = life_of(
        vec![
            obtain("n1", Some("market:buy"), NodeStatus::Ready),
            obtain("n2", Some("market:buy"), NodeStatus::Ready),
            obtain("n3", Some("smith:bronze_bar"), NodeStatus::Ready),
            obtain("n4", Some("market:buy"), NodeStatus::Done),
        ],
        vec![
            entry("n1", "awaiting_fill", 100),
            entry("n2", "awaiting_fill", 100),
            entry("n2", "partial_fill", 150),
            entry("n3", "awaiting_fill", 100),
            entry("n4", "awaiting_fill", 100),
            entry("n9", "launched", 180),
        ],
    );
    assert_eq!(parked_waits(&life), vec![(0, "n1".to_string(), 100)]);
}

fn report(stop: &str, ticks: i64, error: Option<&str>) -> lifescape_json::Value {
    let error_value = match error {
        Some(message) => lifescape_json::object(vec![("type", lifescape_json::str_value("Failed")), ("message", lifescape_json::str_value(message))]),
        None => lifescape_json::Value::Null,
    };
    lifescape_json::object(vec![
        ("node_id", lifescape_json::str_value("fill-n238")),
        ("stop", lifescape_json::str_value(stop)),
        ("ticks", lifescape_json::Value::Int(ticks)),
        ("error", error_value),
    ])
}

/// Live 2026-11-14 (george_97): `melee men --node fill-n238` ended
/// goal_reached after 0 ticks ("no spawn for men" on the mill top), the
/// fold logged "report for unknown node fill-n238, ignored" and the same
/// filler relaunched every 10 s for an hour. A filler report is read:
/// a 0-tick or failed run keeps its skill out of the fillers for a while,
/// a real window does not.
#[test]
fn failed_filler_sits_out_its_skill() {
    use super::{failed_filler_skills, filler_worked, fold_filler_report, FILLER_RETRY_TICKS};
    assert_eq!(filler_worked(&report("goal_reached", 0, None)), Some(false), "0 ticks is no work");
    assert_eq!(filler_worked(&report("failed", 120, Some("no spawn"))), Some(false));
    assert_eq!(filler_worked(&report("budget_exhausted", 1500, None)), Some(true));
    assert_eq!(filler_worked(&report("stopped", 10, None)), None, "operator stop is neutral");
    let mut life = life_of(Vec::new(), vec![entry("fill-n238", "filler:fight:melee/men", 100)]);
    let line = fold_filler_report(&mut life, "fill-n238", &report("goal_reached", 0, None));
    assert!(line.contains("fight:melee/men") && line.contains("failed"), "{line}");
    let fresh = |_: i64| 10u64;
    assert_eq!(failed_filler_skills(&life, &fresh), vec!["attack".to_string()], "melee sits out");
    let stale = |_: i64| FILLER_RETRY_TICKS;
    assert!(failed_filler_skills(&life, &stale).is_empty(), "the cooldown ends");
    let mut working = life_of(Vec::new(), vec![entry("fill-n238", "filler:fight:melee/men", 100)]);
    fold_filler_report(&mut working, "fill-n238", &report("budget_exhausted", 1500, None));
    assert!(failed_filler_skills(&working, &fresh).is_empty(), "a real window never sits out");
}
