//! Goal succession: a life never ends on a finished goal. When a graph
//! finishes (or gets stuck), the brain closes the goal once (history
//! `done:` / `stuck:`, see `bans`) and asks `draw_goal` for the next one,
//! then `append_goal` adds its nodes, climbing from the live levels.
//! Pure: live levels, elapsed ticks and the history come in as arguments.
//!
//! Extension point (per-bot ambitions): `draw_goal` is the single place
//! that picks the next goal; an ambition layer replaces its skill choice
//! (`next_goal`) and must keep honouring the active bans.

use crate::expand::expand_goal_from;
use crate::model::{GoalKind, HistoryEntry, Life, Node, Profile};

/// Skills the socle trains end to end (gather, fight, cook, fire). The
/// order breaks ties before the seed does.
pub const TRAINABLE: &[&str] = &["woodcutting", "mining", "fishing", "attack", "strength", "defence", "cooking", "firemaking"];

/// History tag of a goal that ended stuck (`node: "goal"`): each skill it
/// trained is banned for `bans::SKILL_BAN_TICKS` (`bans::ban_entry`).
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

/// True when the current goal was already closed: the latest entry that
/// is neither graph bookkeeping nor a noted ban expiry is a goal close
/// (`done:` / `stuck:`). A closed goal is never closed twice (live: a
/// finished graph with no goal left re-pushed `done:` on every pass).
pub fn goal_closed(history: &[HistoryEntry]) -> bool {
    history
        .iter()
        .rev()
        .find(|entry| entry.node != "graph" && !(entry.node == "goal" && entry.outcome.starts_with(crate::bans::UNBAN_TAG)))
        .is_some_and(|entry| entry.node == "goal" && (entry.outcome.starts_with(DONE_TAG) || entry.outcome.starts_with(STUCK_TAG)))
}

/// THE goal-drawing entry point, called once the current goal is closed.
/// Money goals are open-ended (closed done, the same goal goes on);
/// otherwise the lowest trainable skill not banned now (`next_goal`).
/// `ticks_since` maps a history stamp to elapsed ticks (bans expire).
/// `None`: every skill is banned or maxed — retried every pass, so a ban
/// expiry alone brings the bot back.
pub fn draw_goal(life: &Life, level_of: &dyn Fn(&str) -> u8, ticks_since: &dyn Fn(i64) -> u64) -> Option<GoalKind> {
    let closed_done = life
        .history
        .iter()
        .rev()
        .find(|entry| entry.node == "goal" && (entry.outcome.starts_with(DONE_TAG) || entry.outcome.starts_with(STUCK_TAG)))
        .is_some_and(|entry| entry.outcome.starts_with(DONE_TAG));
    if closed_done && matches!(life.goal, GoalKind::MakeMoney | GoalKind::GetRich { .. }) {
        return Some(life.goal.clone());
    }
    let avoid = crate::bans::banned_skills(&life.history, ticks_since);
    next_goal(level_of, &avoid, life.seed, life.history.len() as u64)
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
/// Climbs start at the live level (`level_of`): no tranche already held.
pub fn append_goal(life: &mut Life, goal: GoalKind, profile: &Profile, level_of: &dyn Fn(&str) -> u8) {
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
    let fresh: Vec<Node> = expand_goal_from(&goal, profile, level_of)
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
    use super::{append_goal, draw_goal, goal_closed, next_goal, DONE_TAG, STUCK_TAG};
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

    fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
        HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
    }

    fn life_with(goal: GoalKind, history: Vec<HistoryEntry>) -> Life {
        Life {
            schema: 1,
            account: "b0".to_string(),
            created_at: 0,
            seed: 0,
            profile: Profile::new(false, 0.5, 0.5, 0, Archetype::Producer),
            goal,
            nodes: Vec::new(),
            engagement: None,
            active: None,
            history,
        }
    }

    #[test]
    fn a_goal_closes_once_until_work_resumes() {
        assert!(!goal_closed(&[]));
        let mut history = vec![entry("n1", "abandoned", 1), entry("graph", "stuck", 2), entry("goal", &format!("{STUCK_TAG}fishing@6000:x"), 2)];
        assert!(goal_closed(&history));
        history.push(entry("graph", "final", 3));
        history.push(entry("goal", "unban:fishing", 4));
        assert!(goal_closed(&history), "bookkeeping and noted expiries keep it closed");
        history.push(entry("n2", "launched", 5));
        assert!(!goal_closed(&history), "work on a new goal reopens");
    }

    /// The 12 idle bots: every skill stuck by legacy entries. Old bans
    /// without a tick count as expired, so a goal is drawn again; a fresh
    /// ban holds for its ticks only.
    #[test]
    fn bans_expire_and_legacy_bans_never_idle_a_bot() {
        let everything = super::TRAINABLE.join(",");
        let legacy = life_with(GoalKind::MaxSkills { skills: vec!["strength".to_string()], level: 30 }, vec![entry("goal", &format!("{STUCK_TAG}{everything}"), 0)]);
        assert!(draw_goal(&legacy, &|_| 1, &|_| 0).is_some(), "legacy bans are expired");
        let cause = crate::bans::StuckCause { node: "n1".to_string(), class: "3 failures".to_string(), last_error: "x".to_string() };
        let history: Vec<HistoryEntry> = super::TRAINABLE.iter().map(|skill| entry("goal", &crate::bans::ban_entry(skill, 6000, &cause), 100)).collect();
        let banned = life_with(GoalKind::MaxSkills { skills: vec!["strength".to_string()], level: 30 }, history);
        assert_eq!(draw_goal(&banned, &|_| 1, &|at| (5000 - at) as u64), None, "all banned now");
        assert!(draw_goal(&banned, &|_| 1, &|at| (6100 - at) as u64).is_some(), "drawable once the bans run out");
    }

    #[test]
    fn money_goes_on_after_a_done_close_only() {
        let rich = life_with(GoalKind::GetRich { merchant: Some(true) }, vec![entry("goal", DONE_TAG, 0)]);
        assert!(matches!(draw_goal(&rich, &|_| 1, &|_| 0), Some(GoalKind::GetRich { merchant: Some(true) })));
        let stuck = life_with(GoalKind::MakeMoney, vec![entry("goal", &format!("{STUCK_TAG}@0:x"), 0)]);
        assert!(matches!(draw_goal(&stuck, &|_| 1, &|_| 0), Some(GoalKind::MaxSkills { .. })), "a stuck money goal moves to a skill");
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
        append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 10 }, &profile, &|_| 1);
        normalize(&mut life);
        assert_eq!(graph_state(&life), GraphState::Launchable);
        let mut ids: Vec<&str> = life.nodes.iter().map(|node| node.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), life.nodes.len(), "ids stay unique");
    }
}
