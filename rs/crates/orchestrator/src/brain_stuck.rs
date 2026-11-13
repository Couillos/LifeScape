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
use lifescape_planner::{GoalKind, GraphState, HistoryEntry, Life, MAX_REVIVES, REVIVE_COOLDOWN_TICKS};

use crate::brain::push_history;

/// History outcome that ends a spell for good (revive cap reached).
pub const FINAL: &str = "final";

/// State of the stuck spell read back from the history: the last entry
/// that is not goal bookkeeping decides.
#[derive(Debug, Clone, PartialEq)]
pub enum Spell {
    /// No spell open: the graph just turned stuck.
    Fresh,
    /// A spell opened at this unix second, waiting for the revive.
    Waiting(i64),
    /// The revive cap was reached: the operator decides.
    Final,
}

pub fn spell(history: &[HistoryEntry]) -> Spell {
    match history.iter().rev().find(|entry| entry.node != "goal") {
        Some(entry) if entry.node == "graph" && entry.outcome == "stuck" => Spell::Waiting(entry.ended),
        Some(entry) if entry.node == "graph" && entry.outcome == FINAL => Spell::Final,
        _ => Spell::Fresh,
    }
}

/// True when the live snapshot already meets the goal (skill goals: every
/// skill at its target). Other goals answer from the graph, never here.
pub fn goal_met(goal: &GoalKind, live: &Value) -> bool {
    match goal {
        GoalKind::MaxSkills { skills, level } => {
            !skills.is_empty() && skills.iter().all(|skill| crate::brain_succession::live_level(live, skill) >= *level)
        }
        _ => false,
    }
}

/// True when the current goal is reached: the live levels meet a skill
/// goal, or the goal's own root node is done (`goal_root_done`).
pub fn goal_reached(life: &Life, live: &Value) -> bool {
    goal_met(&life.goal, live) || lifescape_planner::goal_root_done(life)
}

/// The graph state the brain acts on: a Stuck graph whose current goal
/// is reached is Finished (closed done, next goal drawn), never a stuck
/// spell. Older goals' abandoned nodes linger in the graph for good; on
/// their own they made every reached goal a "graph stuck" (live
/// 2026-11-13: 540 lines/h over 20 bots; bb4v5d9m's "equip melee" closed
/// stuck every 4 s and its ambition flipped every 30 s).
pub fn effective_state(life: &Life, live: &Value) -> GraphState {
    match lifescape_planner::graph_state(life) {
        GraphState::Stuck { .. } if goal_reached(life, live) => GraphState::Finished,
        state => state,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Wall seconds to D1 ticks (60 ms dev, 600 ms prod).
pub fn seconds_to_ticks(seconds: i64) -> u64 {
    (seconds.max(0) as u64).saturating_mul(1000) / crate::market_eta::live_tick_ms().max(1)
}

/// One stuck pass. Returns true when the life changed (save it).
/// Retry the goal draw of a closed goal (bans may have expired). Returns
/// (a goal was drawn, the life changed).
fn retry_draw(life: &mut Life, live: &Value) -> (bool, bool) {
    let (drawn, changed) = crate::brain_succession::redraw(life, live);
    match drawn {
        Some(label) => {
            crate::logln!("brain {}: stuck spell over, next goal {label}", life.account);
            lifescape_planner::normalize(life);
            (true, true)
        }
        None => (false, changed),
    }
}

pub(crate) fn on_stuck(life: &mut Life, live: &Value, abandoned: &[String]) -> bool {
    match spell(&life.history) {
        Spell::Final => retry_draw(life, live).1,
        Spell::Fresh => {
            let causes: Vec<String> = life
                .nodes
                .iter()
                .filter(|node| abandoned.contains(&node.id))
                .map(|node| format!("{}={}", node.id, if node.last_error.is_empty() { "?" } else { node.last_error.as_str() }))
                .collect();
            crate::logln!("brain {}: graph stuck, nothing left to run; abandoned: {}", life.account, causes.join(" | "));
            push_history(life, "graph", "stuck");
            // A goal the bot already meets is done, whatever older
            // abandoned nodes linger in the graph.
            let stuck = !goal_reached(life, live);
            let drawn = if lifescape_planner::goal_closed(&life.history) {
                crate::brain_succession::redraw(life, live).0
            } else {
                crate::brain_succession::succeed_goal(life, live, stuck)
            };
            match drawn {
                Some(label) => {
                    crate::logln!("brain {}: goal {} -> next goal {label}", life.account, if stuck { "stuck" } else { "done" });
                    lifescape_planner::normalize(life);
                }
                None => crate::logln!(
                    "brain {}: no goal left to draw (see the reason above); the draw retries as bans expire, abandoned nodes retry after {REVIVE_COOLDOWN_TICKS} ticks (at most {MAX_REVIVES} times)",
                    life.account
                ),
            }
            true
        }
        Spell::Waiting(since) => {
            let (drawn, changed) = retry_draw(life, live);
            if drawn {
                return true;
            }
            let waited = seconds_to_ticks(now_secs() - since);
            if waited < REVIVE_COOLDOWN_TICKS {
                return changed;
            }
            let changes = lifescape_planner::revive_stuck(life, waited);
            if changes.is_empty() {
                crate::logln!("brain {}: revive cap reached ({MAX_REVIVES} per node), standing by (operator decides)", life.account);
                push_history(life, "graph", FINAL);
            } else {
                for change in &changes {
                    crate::logln!("brain {}: revive {change}", life.account);
                }
                push_history(life, "graph", "revived");
            }
            true
        }
    }
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
        };
        let live = object(vec![]);
        assert_eq!(super::effective_state(&life, &live), GraphState::Finished);
        life.nodes[1].status = NodeStatus::Abandoned;
        assert!(matches!(super::effective_state(&life, &live), GraphState::Stuck { .. }), "the goal's own step failed");
    }
}
