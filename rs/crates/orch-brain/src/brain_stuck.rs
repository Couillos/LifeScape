#![forbid(unsafe_code)]
//! Stuck graphs in the brain (split from `brain.rs`, rule 4). A stuck
//! spell is the run of passes where nothing is launchable: it opens with
//! one log line and one `graph: stuck` history entry (causes kept in the
//! file), closes the goal once (done when the live levels meet it, stuck
//! otherwise: its skills banned for `SKILL_BAN_TICKS`, cause logged) and
//! draws the next one. With no goal drawable, every pass retries the draw
//! (a ban expiry brings the bot back, `graph: final` included); meanwhile
//! the spell waits for the lifecycle's cooldown revive (`revive_stuck`),
//! and past its cap the abandoned nodes stay down (rule 7).
//!
//! Live 2026-11-12 (bh99tw2f, bbkjz2vk): the spell was re-noted on every
//! pass (the goal entry hid the stuck one: one log line and two history
//! entries every 4 s), every goal reached alongside an older abandoned
//! node was tagged stuck (fishing 15 done, then never trained again),
//! and with every skill "stuck" the bots idled forever.

use lifescape_json::Value;
use lifescape_planner::{GoalKind, GraphState, Life};

// The stuck spell, the effective state and the stuck line are the planner's
// (`upkeep_graph`); `brain_upkeep_drive::graph_state` runs them.
pub use lifescape_planner::upkeep_graph::{spell, stuck_summary, Spell, FINAL};

/// True when the live snapshot already meets the goal (skill goals: every
/// skill at its target). Other goals answer from the graph, never here.
pub fn goal_met(goal: &GoalKind, live: &Value) -> bool {
    lifescape_planner::upkeep_graph::goal_met(goal, &|skill| crate::brain_succession::live_level(live, skill))
}

/// The graph state the brain acts on (a reached goal over older abandoned
/// nodes is Finished, never a stuck spell).
pub fn effective_state(life: &Life, live: &Value) -> GraphState {
    lifescape_planner::upkeep_graph::effective_state(life, &|skill| crate::brain_succession::live_level(live, skill))
}

/// Wall seconds to D1 ticks (60 ms dev, 600 ms prod).
pub fn seconds_to_ticks(seconds: i64) -> u64 {
    (seconds.max(0) as u64).saturating_mul(1000) / crate::market_eta::live_tick_ms().max(1)
}

#[cfg(test)]
mod tests {
    use super::{goal_met, spell, Spell, FINAL};
    use lifescape_json::{object, Value};
    use lifescape_planner::{GoalKind, HistoryEntry};

    fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
        HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
    }

    /// The goal entry pushed after the stuck one never reopens the spell
    /// (live: one stuck line and two history entries every pass).
    #[test]
    fn a_spell_is_noted_once_and_ends_on_any_node_event() {
        assert_eq!(spell(&[]), Spell::Fresh);
        let mut history = vec![entry("n24", "abandoned", 10), entry("graph", "stuck", 11), entry("goal", "stuck:woodcutting", 11)];
        assert_eq!(spell(&history), Spell::Waiting(11), "the goal bookkeeping keeps the spell open");
        history.push(entry("graph", "revived", 20));
        assert_eq!(spell(&history), Spell::Fresh, "a revive ends the spell");
        history.push(entry("graph", "stuck", 30));
        history.push(entry("graph", FINAL, 40));
        assert_eq!(spell(&history), Spell::Final);
        history.push(entry("n30", "launched", 50));
        assert_eq!(spell(&history), Spell::Fresh, "operator work reopens the cycle");
    }

    /// bh99tw2f: fishing 15 reached (live 16) was tagged stuck because
    /// older nodes were abandoned; a met skill goal is done.
    #[test]
    fn a_met_skill_goal_is_done_whatever_lingers() {
        let live = object(vec![(
            "skills",
            Value::Arr(vec![object(vec![("id", Value::Int(10)), ("level", Value::Int(16))])]),
        )]);
        assert!(goal_met(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 15 }, &live));
        assert!(!goal_met(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 }, &live));
        assert!(!goal_met(&GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 15 }, &live), "unknown = level 1");
        assert!(!goal_met(&GoalKind::MakeMoney, &live), "non-skill goals answer from the graph");
    }

    /// bst29bh9 (live 2026-11-14): ~90 abandoned nodes, one cascade line
    /// each, on one log line. The line names the count and the roots only.
    #[test]
    fn the_stuck_line_stays_bounded() {
        use lifescape_planner::{Life, Node, NodeKind, NodeStatus};
        let mut nodes = Vec::new();
        for number in 1..=200 {
            let mut node = Node::new(&format!("n{number}"), NodeKind::ObtainItems { item: "logs".to_string(), count: 1 }, Vec::new(), 1);
            node.status = NodeStatus::Abandoned;
            node.last_error = if number % 40 == 1 { format!("Failed: {}", "x".repeat(500)) } else { format!("dependency n{} abandoned", number - 1) };
            nodes.push(node);
        }
        let ids: Vec<String> = nodes.iter().map(|node| node.id.clone()).collect();
        let life = Life {
            schema: 1,
            account: "b0".to_string(),
            created_at: 0,
            seed: 0,
            profile: lifescape_planner::Profile::new(false, 0.5, 0.5, 0, lifescape_planner::Archetype::Producer),
            goal: GoalKind::MakeMoney,
            nodes,
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        let line = super::stuck_summary(&life, &ids);
        assert!(line.starts_with("200 nodes (195 by cascade); roots: n1=Failed: "), "{line}");
        assert!(line.ends_with("(+2 more roots)"), "{line}");
        assert!(line.len() < 700, "bounded: {} chars", line.len());
    }

    /// bb4v5d9m (live 2026-11-13): an old abandoned obtain plus a done
    /// "equip melee" step is a finished goal, not a stuck spell; with the
    /// goal's own step abandoned the spell stands.
    #[test]
    fn a_reached_goal_over_old_abandons_is_finished_not_stuck() {
        use lifescape_planner::{GraphState, Life, Node, NodeKind, NodeStatus};
        let mut old = Node::new("n51", NodeKind::ObtainItems { item: "leather_boots".to_string(), count: 1 }, Vec::new(), 1);
        old.status = NodeStatus::Abandoned;
        let mut equip = Node::new("n457", NodeKind::EquipBest { style: "melee".to_string() }, Vec::new(), 1);
        equip.status = NodeStatus::Done;
        let mut life = Life {
            schema: 1,
            account: "b0".to_string(),
            created_at: 0,
            seed: 0,
            profile: lifescape_planner::Profile::new(false, 0.5, 0.5, 0, lifescape_planner::Archetype::Producer),
            goal: GoalKind::EquipBest { style: "melee".to_string() },
            nodes: vec![old, equip],
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        let live = object(vec![]);
        assert_eq!(super::effective_state(&life, &live), GraphState::Finished);
        life.nodes[1].status = NodeStatus::Abandoned;
        assert!(matches!(super::effective_state(&life, &live), GraphState::Stuck { .. }), "the goal's own step failed");
    }
}
