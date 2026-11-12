//! Goal succession: a life never ends on a finished goal. When a graph
//! finishes (or gets stuck), the brain asks for the next goal here and
//! appends its nodes. Pure: live levels and the history come in as
//! arguments.

use crate::expand::expand_goal;
use crate::model::{GoalKind, HistoryEntry, Life, Node, Profile};

/// Skills the socle trains end to end (gather, fight, cook, fire). The
/// order breaks ties before the seed does.
pub const TRAINABLE: &[&str] = &["woodcutting", "mining", "fishing", "attack", "strength", "defence", "cooking", "firemaking"];

/// History tag of a goal that ended stuck (`node: "goal"`): the skills it
/// trained are skipped by `next_goal` so a dead goal is never redrawn.
pub const STUCK_TAG: &str = "stuck:";
/// History tag of a goal that ended done.
pub const DONE_TAG: &str = "done:";

/// Skills named by a goal (empty for non-skill goals).
pub fn goal_skills(goal: &GoalKind) -> Vec<String> {
    match goal {
        GoalKind::MaxSkills { skills, .. } => skills.clone(),
        _ => Vec::new(),
    }
}

/// Skills whose goals already ended stuck in this life.
pub fn stuck_skills(history: &[HistoryEntry]) -> Vec<String> {
    history
        .iter()
        .filter(|entry| entry.node == "goal")
        .filter_map(|entry| entry.outcome.strip_prefix(STUCK_TAG))
        .flat_map(|skills| skills.split(',').map(str::to_string))
        .filter(|skill| !skill.is_empty())
        .collect()
}

/// Next goal: the lowest trainable skill not stuck before, trained to the
/// next multiple of 5 at least 5 levels up (1 -> 10, 17 -> 25), capped at
/// 99. Ties go to the seed-rotated order, so bots do not all pick the
/// same skill. `None` when every trainable skill is stuck or maxed.
pub fn next_goal(level_of: &dyn Fn(&str) -> u8, avoid: &[String], seed: u64, counter: u64) -> Option<GoalKind> {
    let rotation = (seed.wrapping_add(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)) % TRAINABLE.len() as u64) as usize;
    let mut best: Option<(u8, usize, &str)> = None;
    for (index, skill) in TRAINABLE.iter().enumerate() {
        if avoid.iter().any(|stuck| stuck == skill) {
            continue;
        }
        let level = level_of(skill).max(1);
        if level >= 99 {
            continue;
        }
        let order = (index + TRAINABLE.len() - rotation) % TRAINABLE.len();
        if best.is_none_or(|(known, known_order, _)| (level, order) < (known, known_order)) {
            best = Some((level, order, skill));
        }
    }
    let (level, _, skill) = best?;
    let target = ((u16::from(level) + 10) / 5 * 5).min(99) as u8;
    Some(GoalKind::MaxSkills { skills: vec![skill.to_string()], level: target })
}

/// Replace the life's goal and append the new goal's nodes, renumbered
/// past every existing id (old nodes stay as the record of past goals).
pub fn append_goal(life: &mut Life, goal: GoalKind, profile: &Profile) {
    let offset = life
        .nodes
        .iter()
        .filter_map(|node| node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()))
        .max()
        .unwrap_or(0)
        .max(life.nodes.len());
    let renumber = |id: &str| -> String {
        match id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()) {
            Some(number) => format!("n{}", number + offset),
            None => id.to_string(),
        }
    };
    let fresh: Vec<Node> = expand_goal(&goal, profile)
        .into_iter()
        .map(|mut node| {
            node.id = renumber(&node.id);
            node.deps = node.deps.iter().map(|dep| renumber(dep)).collect();
            node
        })
        .collect();
    life.nodes.extend(fresh);
    life.goal = goal;
    life.active = None;
}

#[cfg(test)]
mod tests {
    use super::{append_goal, next_goal, stuck_skills, STUCK_TAG};
    use crate::lifecycle::{graph_state, normalize, GraphState};
    use crate::model::{Archetype, GoalKind, HistoryEntry, Life, NodeStatus, Profile};

    fn levels<'pairs>(pairs: &'pairs [(&'pairs str, u8)]) -> impl Fn(&str) -> u8 + 'pairs {
        move |skill| pairs.iter().find(|(name, _)| *name == skill).map(|(_, level)| *level).unwrap_or(1)
    }

    #[test]
    fn next_goal_trains_the_lowest_unstuck_skill_to_the_next_step() {
        let woodcutter = levels(&[("woodcutting", 17), ("mining", 16), ("fishing", 5), ("attack", 3), ("strength", 3), ("defence", 3), ("cooking", 2), ("firemaking", 2)]);
        let goal = next_goal(&woodcutter, &["cooking".to_string(), "firemaking".to_string()], 0, 0).expect("a goal");
        assert!(matches!(goal, GoalKind::MaxSkills { ref skills, level: 10 } if skills == &vec!["attack".to_string()]), "got {goal:?}");
        let high = levels(&[("woodcutting", 17)]);
        let all_but_wood: Vec<String> = super::TRAINABLE.iter().filter(|skill| **skill != "woodcutting").map(|skill| skill.to_string()).collect();
        let goal = next_goal(&high, &all_but_wood, 0, 0).expect("woodcutting left");
        assert!(matches!(goal, GoalKind::MaxSkills { level: 25, .. }), "17 -> 25, got {goal:?}");
        let everything: Vec<String> = super::TRAINABLE.iter().map(|skill| skill.to_string()).collect();
        assert_eq!(next_goal(&high, &everything, 0, 0), None, "all stuck: the operator decides");
    }

    #[test]
    fn stuck_history_is_read_back() {
        let history = vec![
            HistoryEntry { node: "goal".to_string(), started: 0, ended: 0, outcome: format!("{STUCK_TAG}fishing,cooking") },
            HistoryEntry { node: "n1".to_string(), started: 0, ended: 0, outcome: "failed".to_string() },
        ];
        assert_eq!(stuck_skills(&history), vec!["fishing".to_string(), "cooking".to_string()]);
    }

    /// A finished life gets a fresh, launchable graph with unique ids.
    #[test]
    fn append_goal_renumbers_and_relaunches() {
        let profile = Profile::new(false, 0.5, 0.5, 3, Archetype::Producer);
        let first = GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 15 };
        let mut life = Life {
            schema: 1,
            account: "b0".to_string(),
            created_at: 0,
            seed: 3,
            profile: profile.clone(),
            goal: first.clone(),
            nodes: crate::expand::expand_goal(&first, &profile),
            engagement: None,
            active: None,
            history: Vec::new(),
        };
        for node in &mut life.nodes {
            node.status = NodeStatus::Done;
        }
        assert_eq!(graph_state(&life), GraphState::Finished);
        append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 10 }, &profile);
        normalize(&mut life);
        assert_eq!(graph_state(&life), GraphState::Launchable);
        let mut ids: Vec<&str> = life.nodes.iter().map(|node| node.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), life.nodes.len(), "ids stay unique");
    }
}
