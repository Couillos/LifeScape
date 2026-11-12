#![forbid(unsafe_code)]
//! Goal / task columns of the spectator bot table: one compact summary of
//! `run/lives/<account>.json` per listed job. The job list rides the 1 Hz
//! WS frame, so each file is parsed only when its mtime (or size) moves;
//! otherwise the cached summary is served after a single `stat`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use lifescape_planner::{graph_state, normalize, GoalKind, GraphState, Life, NodeKind, NodeStatus};

/// Display cells for one bot: long-term goal, current task and the task
/// tooltip (node kind/progress, or the abandoned nodes' errors).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LifeSummary {
    pub goal: String,
    pub task: String,
    pub task_title: String,
}

/// Compact goal label: `fishing 20`, `obtain logs 28`, `quest sheep`,
/// `money`, `rich`.
pub(crate) fn goal_text(goal: &GoalKind) -> String {
    match goal {
        GoalKind::MaxSkills { skills, level } => format!("{} {level}", skills.join(",")),
        GoalKind::Obtain { item, count } => format!("obtain {item} {count}"),
        GoalKind::MakeMoney => "money".to_string(),
        GoalKind::CompleteQuest { quest } => format!("quest {quest}"),
        GoalKind::EquipBest { style } => format!("equip {style}"),
        GoalKind::AccessArea { area } => format!("area {area}"),
        GoalKind::GetRich { merchant: None } => "rich".to_string(),
        GoalKind::GetRich { merchant: Some(true) } => "rich merchant".to_string(),
        GoalKind::GetRich { merchant: Some(false) } => "rich producer".to_string(),
    }
}

fn kind_text(kind: &NodeKind) -> String {
    match kind {
        NodeKind::ReachLevel { skill, level } => format!("reach_level {skill} {level}"),
        NodeKind::ReachCombat { level } => format!("reach_combat {level}"),
        NodeKind::ObtainItems { item, count } => format!("obtain_items {item} {count}"),
        NodeKind::HaveCoins { amount } => format!("have_coins {amount}"),
        NodeKind::MakeMoney => "make_money".to_string(),
        NodeKind::CompleteQuest { quest } => format!("complete_quest {quest}"),
        NodeKind::EquipBest { style } => format!("equip_best {style}"),
        NodeKind::AccessArea { area } => format!("access_area {area}"),
        NodeKind::SpaceCleanup => "space_cleanup".to_string(),
    }
}

/// Task cell of a life, computed on a normalized copy (the file itself is
/// never rewritten here): `n1 gather:fishing/raw_shrimp` for the active
/// node, `done` once every node is done, `stuck` (abandoned errors in the
/// tooltip) when nothing is left to launch, `ready` between launches.
pub(crate) fn summarize(life: &Life) -> LifeSummary {
    let mut graph = life.clone();
    normalize(&mut graph);
    let (task, task_title) = match graph_state(&graph) {
        GraphState::Running(active) => match graph.nodes.iter().find(|node| node.id == active) {
            Some(node) => {
                let label = match node.method.as_deref() {
                    Some(method) if !method.is_empty() => method.to_string(),
                    _ => kind_text(&node.kind),
                };
                let title = format!(
                    "{} · progress {}/{}",
                    kind_text(&node.kind),
                    node.progress_current,
                    node.progress_target
                );
                (format!("{} {label}", node.id), title)
            }
            None => (active, String::new()),
        },
        GraphState::Finished => ("done".to_string(), String::new()),
        GraphState::Launchable => {
            let ready: Vec<&str> = graph
                .nodes
                .iter()
                .filter(|node| node.status == NodeStatus::Ready)
                .map(|node| node.id.as_str())
                .collect();
            ("ready".to_string(), format!("next: {}", ready.join(", ")))
        }
        GraphState::Stuck { abandoned } => {
            let lines: Vec<String> = graph
                .nodes
                .iter()
                .filter(|node| abandoned.contains(&node.id))
                .map(|node| {
                    let error = if node.last_error.is_empty() { "no error recorded" } else { node.last_error.as_str() };
                    format!("{} {}: {error}", node.id, kind_text(&node.kind))
                })
                .collect();
            let title = if lines.is_empty() { "nothing launchable".to_string() } else { lines.join("\n") };
            ("stuck".to_string(), title)
        }
    };
    LifeSummary { goal: goal_text(&graph.goal), task, task_title }
}

type Stamp = (SystemTime, u64);

fn cache() -> &'static Mutex<HashMap<String, (Stamp, Option<LifeSummary>)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Stamp, Option<LifeSummary>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Summary for one account, `None` without a life file (manual bot). A
/// file that exists but does not parse shows `unreadable`, never a guess.
pub(crate) fn summary_for(account: &str) -> Option<LifeSummary> {
    if account.is_empty() || account == "." || account == ".." || account.contains('/') {
        return None;
    }
    let path = crate::brain::life_path(account);
    let Ok(meta) = std::fs::metadata(&path) else {
        if let Ok(mut entries) = cache().lock() {
            entries.remove(account);
        }
        return None;
    };
    let stamp: Stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
    if let Ok(entries) = cache().lock() {
        if let Some((cached, summary)) = entries.get(account) {
            if *cached == stamp {
                return summary.clone();
            }
        }
    }
    let summary = match crate::brain::load_life(account) {
        Some(life) => summarize(&life),
        None => LifeSummary {
            goal: "unreadable".to_string(),
            task: "unreadable".to_string(),
            task_title: format!("{} does not parse as a life", path.display()),
        },
    };
    if let Ok(mut entries) = cache().lock() {
        entries.insert(account.to_string(), (stamp, Some(summary.clone())));
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_planner::life_from_value;

    fn node(id: &str, kind: &str, params: &str, status: &str, method: &str, last_error: &str) -> String {
        format!(
            r#"{{"id": "{id}", "kind": "{kind}", "params": {params}, "deps": [], "status": "{status}",
               "method": {method}, "alternatives": [], "progress": {{"current": 3, "target": 20}},
               "spent_ticks": 0, "failures": 0, "blocks": 0, "detail": "", "last_error": "{last_error}"}}"#,
            method = if method.is_empty() { "null".to_string() } else { format!("\"{method}\"") },
        )
    }

    fn life(goal: &str, nodes: &[String], active: Option<&str>) -> Life {
        let text = format!(
            r#"{{"schema": 1, "account": "t01", "created_at": 0, "seed": 1, "goal": {goal},
               "profile": {{"archetype": "producer", "ironman": false, "market_affinity": 0.5, "price_patience": 0.5}},
               "nodes": [{}], "engagement": null, "active": {}, "history": []}}"#,
            nodes.join(","),
            active.map(|id| format!("\"{id}\"")).unwrap_or_else(|| "null".to_string()),
        );
        life_from_value(&lifescape_json::parse(&text).expect("fixture json")).expect("fixture life")
    }

    #[test]
    fn goal_labels_are_compact() {
        let skills = GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 };
        assert_eq!(goal_text(&skills), "fishing 20");
        assert_eq!(goal_text(&GoalKind::Obtain { item: "logs".to_string(), count: 28 }), "obtain logs 28");
        assert_eq!(goal_text(&GoalKind::CompleteQuest { quest: "sheep".to_string() }), "quest sheep");
        assert_eq!(goal_text(&GoalKind::MakeMoney), "money");
        assert_eq!(goal_text(&GoalKind::GetRich { merchant: None }), "rich");
        assert_eq!(goal_text(&GoalKind::GetRich { merchant: Some(true) }), "rich merchant");
    }

    #[test]
    fn active_node_shows_id_and_method() {
        let goal = r#"{"kind": "max_skills", "skills": ["fishing"], "level": 20}"#;
        let params = r#"{"skill": "fishing", "level": 20}"#;
        let nodes = [node("n1", "reach_level", params, "active", "gather:fishing/raw_shrimp", "")];
        let summary = summarize(&life(goal, &nodes, Some("n1")));
        assert_eq!(summary.goal, "fishing 20");
        assert_eq!(summary.task, "n1 gather:fishing/raw_shrimp");
        assert_eq!(summary.task_title, "reach_level fishing 20 · progress 3/20");
    }

    #[test]
    fn finished_graph_is_done() {
        let goal = r#"{"kind": "complete_quest", "quest": "sheep"}"#;
        let nodes = [node("n1", "complete_quest", r#"{"quest": "sheep"}"#, "done", "quest:sheep", "")];
        let summary = summarize(&life(goal, &nodes, None));
        assert_eq!((summary.goal.as_str(), summary.task.as_str()), ("quest sheep", "done"));
    }

    #[test]
    fn abandoned_graph_is_stuck_with_errors_in_title() {
        let goal = r#"{"kind": "obtain", "item": "logs", "count": 28}"#;
        let nodes = [
            node("n1", "obtain_items", r#"{"item": "logs", "count": 28}"#, "abandoned", "", "Blocked: no axe"),
            node("n2", "complete_quest", r#"{"quest": "tutorial"}"#, "done", "tutorial", ""),
        ];
        let summary = summarize(&life(goal, &nodes, None));
        assert_eq!(summary.goal, "obtain logs 28");
        assert_eq!(summary.task, "stuck");
        assert_eq!(summary.task_title, "n1 obtain_items logs 28: Blocked: no axe");
    }
}
