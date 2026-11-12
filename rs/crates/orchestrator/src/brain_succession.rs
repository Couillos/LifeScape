//! Goal succession in the brain: a finished graph draws the next goal, a
//! stuck graph records why (history `goal: stuck:<skills>` plus every
//! abandoned node's `last_error`, kept in the file) and moves on to a
//! goal that does not retrain the stuck skills. Only when no goal is left
//! does the bot stand by for the operator.

#![forbid(unsafe_code)]

use lifescape_json::Value;
use lifescape_planner::{append_goal, goal_skills, next_goal, stuck_skills, GoalKind, Life, DONE_TAG, STUCK_TAG};

use crate::brain::push_history;

/// Live level of one skill from the snapshot (`skills: [{id, level}]`).
pub(crate) fn live_level(live: &Value, skill: &str) -> u8 {
    let Some(stat) = lifescape_sdk::provenance::skill_id(skill) else { return 1 };
    live.get("skills")
        .as_arr()
        .and_then(|rows| rows.iter().find(|row| row.get("id").as_i64() == Some(stat as i64)).and_then(|row| row.get("level").as_i64()))
        .map(|level| level.clamp(1, 99) as u8)
        .unwrap_or(1)
}

fn goal_label(goal: &GoalKind) -> String {
    match goal {
        GoalKind::MaxSkills { skills, level } => format!("{} {level}", skills.join(",")),
        other => format!("{other:?}"),
    }
}

/// Close the current goal (done or stuck) and append the next one.
/// Returns the new goal's label, or `None` when nothing is left.
pub(crate) fn succeed_goal(life: &mut Life, live: &Value, stuck: bool) -> Option<String> {
    let skills = goal_skills(&life.goal).join(",");
    let tag = if stuck { STUCK_TAG } else { DONE_TAG };
    push_history(life, "goal", &format!("{tag}{skills}"));
    // Money goals are open-ended: one engagement done, the next one starts.
    let next = match &life.goal {
        GoalKind::MakeMoney | GoalKind::GetRich { .. } if !stuck => Some(life.goal.clone()),
        _ => {
            let avoid = stuck_skills(&life.history);
            let level_of = |skill: &str| live_level(live, skill);
            next_goal(&level_of, &avoid, life.seed, life.history.len() as u64)
        }
    }?;
    let label = goal_label(&next);
    let profile = life.profile.clone();
    append_goal(life, next, &profile);
    Some(label)
}

#[cfg(test)]
mod tests {
    use super::{live_level, succeed_goal};
    use lifescape_json::{object, Value};
    use lifescape_planner::{Archetype, GoalKind, Life, Profile};

    fn live(levels: &[(i64, i64)]) -> Value {
        object(vec![(
            "skills",
            Value::Arr(levels.iter().map(|(id, level)| object(vec![("id", Value::Int(*id)), ("level", Value::Int(*level))])).collect()),
        )])
    }

    fn life_with(goal: GoalKind) -> Life {
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
            history: Vec::new(),
        }
    }

    #[test]
    fn done_woodcutter_moves_on_and_stuck_fisher_never_redraws_fishing() {
        let snapshot = live(&[(8, 17), (14, 16), (10, 5)]);
        assert_eq!(live_level(&snapshot, "woodcutting"), 17);
        let mut woodcutter = life_with(GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 15 });
        let label = succeed_goal(&mut woodcutter, &snapshot, false).expect("a next goal");
        assert!(!label.starts_with("woodcutting"), "the lowest skill, not the one just done, got {label}");
        assert!(!woodcutter.nodes.is_empty(), "the new goal brings nodes");
        let mut fisher = life_with(GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 10 });
        for _ in 0..5 {
            let label = succeed_goal(&mut fisher, &live(&[]), true).unwrap_or_default();
            assert!(!label.starts_with("fishing"), "a stuck skill is never redrawn, got {label}");
            fisher.goal = GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 10 };
        }
    }

    #[test]
    fn money_goals_restart_their_engagement() {
        let mut rich = life_with(GoalKind::GetRich { merchant: Some(true) });
        succeed_goal(&mut rich, &live(&[]), false).expect("money goes on");
        assert!(matches!(rich.goal, GoalKind::GetRich { merchant: Some(true) }));
    }
}
