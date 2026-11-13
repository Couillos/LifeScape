//! Goal succession in the brain: a finished graph draws the next goal, a
//! stuck graph records why and bans the goal's skills for
//! `SKILL_BAN_TICKS` only (user decision 2026-11-13: never for life).
//! Every ban is logged with its cause (root node, class, last error) and
//! written to the history (`stuck:<skill>@<ticks>:<cause>`); its expiry is
//! logged once (`unban:<skill>`). Old `stuck:<skills>` entries carry no
//! tick and count as expired. With every skill banned the bot waits; each
//! pass retries the draw, so a ban expiry alone brings it back.
//!
//! The pure decisions live in the planner (`goals::draw_goal_origin`, the
//! single goal-drawing entry point, `bans` and `ambition`); this file only
//! stamps, logs and appends. Goals come from the life's ambition
//! (`brain_ambition`): each drawn goal records `goal: next:<label>` and
//! `life.goal_origin` (its milestone); the plain lowest-skill draw serves
//! only the explicit `generalist` ambition.

#![forbid(unsafe_code)]

use lifescape_json::Value;
use lifescape_planner::{
    append_goal, ban_entry, ban_states, draw_goal_origin, goal_closed, mark_drawn, goal_skills, stuck_cause, unban_entry, BanState, GoalKind, Life,
    StuckCause, DONE_TAG, SKILL_BAN_TICKS,
};

use crate::brain::push_history;

/// History tag of a drawn goal (`goal: next:<label>`).
pub const NEXT_TAG: &str = "next:";

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
        GoalKind::Produce { item, count } => format!("produce {item} {count}"),
        GoalKind::EquipBest { style } => format!("equip {style}"),
        GoalKind::GetRich { merchant: Some(true) } => "rich merchant".to_string(),
        other => format!("{other:?}"),
    }
}

/// Ticks elapsed since a history stamp (unix seconds), with the daemon's
/// measured tick length (60 ms dev, 600 ms prod).
fn ticks_since(stamp: i64) -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    crate::brain_stuck::seconds_to_ticks(now - stamp)
}

/// Close the current goal once: `done:<skills>`, or one ban per trained
/// skill (stuck), each logged with its cause; then fold the close into
/// the goal's milestone (`brain_ambition::after_close`).
pub(crate) fn close_goal(life: &mut Life, live: &Value, stuck: bool) {
    close_goal_history(life, stuck);
    crate::brain_ambition::after_close(life, live, stuck);
}

fn close_goal_history(life: &mut Life, stuck: bool) {
    let skills = goal_skills(&life.goal);
    if !stuck {
        push_history(life, "goal", &format!("{DONE_TAG}{}", skills.join(",")));
        return;
    }
    let cause = stuck_cause(life).unwrap_or(StuckCause {
        node: "?".to_string(),
        class: "graph stuck".to_string(),
        last_error: "no abandoned node found".to_string(),
    });
    if skills.is_empty() {
        crate::logln!("brain {}: goal {} stuck: {}", life.account, goal_label(&life.goal), cause.describe());
        push_history(life, "goal", &ban_entry("", 0, &cause));
        return;
    }
    for skill in &skills {
        crate::logln!("brain {}: skill {skill} banned {SKILL_BAN_TICKS} ticks: {} (node {}, {})", life.account, cause.class, cause.node, cause.last_error);
        push_history(life, "goal", &ban_entry(skill, SKILL_BAN_TICKS, &cause));
    }
}

/// Note every ban that ran out (one log line and one `unban:` entry per
/// skill). Returns true when the history changed.
pub(crate) fn note_expired_bans(life: &mut Life) -> bool {
    let mut changed = false;
    for (skill, state) in ban_states(&life.history, &ticks_since) {
        if let BanState::Expired { cause, legacy } = state {
            if legacy {
                crate::logln!("brain {}: skill {skill} ban lifted (old entry without tick counts as expired: {cause})", life.account);
            } else {
                crate::logln!("brain {}: skill {skill} ban expired after {SKILL_BAN_TICKS} ticks (was: {cause})", life.account);
            }
            push_history(life, "goal", &unban_entry(&skill));
            changed = true;
        }
    }
    changed
}

/// Last "nothing drawable" reason logged per account (one line per
/// change, not one per pass).
fn last_wait() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Draw and append the next goal for a closed one (expired bans noted,
/// milestones refreshed first). Returns the new goal's label, or `None`
/// while nothing is drawable (typed reason logged once per change).
pub(crate) fn draw_next(life: &mut Life, live: &Value) -> Option<String> {
    note_expired_bans(life);
    crate::brain_ambition::before_draw(life, live);
    let level_of = |skill: &str| live_level(live, skill);
    let (next, origin) = match draw_goal_origin(life, &level_of, &ticks_since) {
        Ok(drawn) => drawn,
        Err(reason) => {
            let text = reason.describe();
            let mut table = last_wait().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if table.get(&life.account) != Some(&text) {
                crate::logln!("brain {}: no goal drawn: {text}", life.account);
                table.insert(life.account.clone(), text);
            }
            return None;
        }
    };
    last_wait().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&life.account);
    let label = goal_label(&next);
    let profile = life.profile.clone();
    append_goal(life, next, &profile, &level_of);
    mark_drawn(life, origin.as_deref());
    if let Some(id) = &origin {
        crate::logln!("brain {}: goal {label} from milestone {id}", life.account);
    }
    // Opens the new goal in the history (`goal_closed` reads false), so
    // a goal met at once is closed before the next draw.
    push_history(life, "goal", &format!("{NEXT_TAG}{label}"));
    Some(label)
}

/// Close the current goal (done or stuck) and append the next one.
/// Returns the new goal's label, or `None` when nothing is left.
pub(crate) fn succeed_goal(life: &mut Life, live: &Value, stuck: bool) -> Option<String> {
    close_goal(life, live, stuck);
    draw_next(life, live)
}

/// A goal closed earlier with nothing drawable then: retry the draw (a
/// ban may have expired). Returns (label of the new goal, life changed).
pub(crate) fn redraw(life: &mut Life, live: &Value) -> (Option<String>, bool) {
    if !goal_closed(&life.history) {
        return (None, false);
    }
    let before = life.history.len();
    let ambition_before = life.ambition.clone();
    let drawn = draw_next(life, live);
    let changed = drawn.is_some() || life.history.len() != before || life.ambition != ambition_before;
    (drawn, changed)
}

#[cfg(test)]
mod tests {
    use super::{live_level, redraw, succeed_goal};
    use lifescape_json::{object, Value};
    use lifescape_planner::{
        new_ambition, Archetype, GoalKind, HistoryEntry, Life, MilestoneStatus, Node, NodeKind, NodeStatus, Profile, Stamp, GENERALIST, STUCK_TAG,
    };

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
            // The explicit generalist keeps the plain lowest-skill draw.
            ambition: new_ambition(GENERALIST, "operator", &|_| 1, 0, &[], Stamp { tick: 0, secs: 0 }),
            goal_origin: None,
        }
    }

    /// An ambitious bot draws its next milestone, linked by
    /// `goal_origin`; a done quota closes its milestone; no ambition
    /// draws nothing (never a silent lowest-skill fallback).
    #[test]
    fn goals_follow_the_ambition_milestones() {
        let mut smith = life_with(GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 5 });
        // Drawn now: the optional change stays throttled (no fleet read).
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
        smith.ambition = new_ambition("smith", "coverage:smithing", &|_| 1, 3, &[], Stamp { tick: 0, secs: now });
        // Mining 12, smithing 1: the mining 10 milestone is met at refresh.
        let snapshot = live(&[(14, 12)]);
        let label = succeed_goal(&mut smith, &snapshot, false).expect("a goal");
        assert_eq!(label, "smithing 10", "next open milestone after the met mining 10");
        let origin = smith.goal_origin.clone().expect("linked");
        let ambition = smith.ambition.as_ref().expect("smith");
        assert_eq!(ambition.milestones.iter().find(|stone| stone.id == origin).map(|stone| stone.status), Some(MilestoneStatus::Active));
        assert!(smith.history.iter().any(|entry| entry.outcome == "next:smithing 10"));
        let mining = ambition.milestones.iter().find(|stone| stone.label == "mining 10").expect("mining milestone");
        assert_eq!((mining.status, mining.progress), (MilestoneStatus::Done, (10, 10)));
        // Skip to the quota: smithing 10 met, the bronze quota is drawn.
        let label = succeed_goal(&mut smith, &live(&[(14, 12), (13, 10)]), false).expect("the quota");
        assert!(label.starts_with("produce bronze_"), "got {label}");
        assert!(matches!(smith.goal, GoalKind::Produce { count: 20, .. }));
        let quota = smith.goal_origin.clone().expect("quota link");
        succeed_goal(&mut smith, &live(&[(14, 12), (13, 10)]), false).expect("next");
        let done = smith.ambition.as_ref().and_then(|ambition| ambition.milestones.iter().find(|stone| stone.id == quota).cloned()).expect("quota");
        assert_eq!(done.status, MilestoneStatus::Done);
        let mut bare = life_with(GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 5 });
        bare.ambition = None;
        assert_eq!(succeed_goal(&mut bare, &snapshot, false), None, "no ambition, no goal");
    }

    #[test]
    fn done_woodcutter_moves_on_and_a_banned_fisher_never_redraws_fishing() {
        let snapshot = live(&[(8, 17), (14, 16), (10, 5)]);
        assert_eq!(live_level(&snapshot, "woodcutting"), 17);
        let mut woodcutter = life_with(GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 15 });
        let label = succeed_goal(&mut woodcutter, &snapshot, false).expect("a next goal");
        assert!(!label.starts_with("woodcutting"), "the lowest skill, not the one just done, got {label}");
        assert!(!woodcutter.nodes.is_empty(), "the new goal brings nodes");
        let mut fisher = life_with(GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 10 });
        for _ in 0..5 {
            let label = succeed_goal(&mut fisher, &live(&[]), true).unwrap_or_default();
            assert!(!label.starts_with("fishing"), "a banned skill is not redrawn while banned, got {label}");
            fisher.goal = GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 10 };
        }
        let ban = fisher.history.iter().find(|entry| entry.outcome.starts_with("stuck:fishing@6000:")).expect("typed ban entry");
        assert!(ban.outcome.contains("graph stuck"), "the cause rides in the entry, got {}", ban.outcome);
    }

    /// The new goal climbs from the live level (attack 45 -> one tranche).
    #[test]
    fn the_next_goal_climbs_from_the_live_level() {
        let mut life = life_with(GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 15 });
        // Every skill high except attack at 45: attack 55 is drawn.
        let snapshot = live(&[(0, 45), (1, 60), (2, 60), (7, 60), (8, 60), (10, 60), (11, 60), (14, 60)]);
        let label = succeed_goal(&mut life, &snapshot, false).expect("a goal");
        assert_eq!(label, "attack 55");
        let climbs: Vec<u8> = life.nodes.iter().filter_map(|node| match node.kind {
            NodeKind::ReachLevel { level, .. } => Some(level),
            _ => None,
        }).collect();
        assert_eq!(climbs, vec![55], "one tranche 45 -> 55");
    }

    #[test]
    fn money_goals_restart_their_engagement() {
        let mut rich = life_with(GoalKind::GetRich { merchant: Some(true) });
        succeed_goal(&mut rich, &live(&[]), false).expect("money goes on");
        assert!(matches!(rich.goal, GoalKind::GetRich { merchant: Some(true) }));
    }

    /// The 12 idle bots (`graph: final`, every skill in an old
    /// `stuck:<skills>` entry): the redraw lifts the legacy bans (one
    /// `unban:` per skill) and gives them a goal again.
    #[test]
    fn legacy_banned_bots_get_a_goal_again() {
        let mut life = life_with(GoalKind::MaxSkills { skills: vec!["strength".to_string()], level: 30 });
        let mut done = Node::new("n1", NodeKind::ReachLevel { skill: "strength".to_string(), level: 30 }, Vec::new(), 30);
        done.status = NodeStatus::Done;
        life.nodes.push(done);
        let every = lifescape_planner::TRAINABLE.join(",");
        life.history = vec![
            HistoryEntry { node: "goal".to_string(), started: 0, ended: 0, outcome: format!("{STUCK_TAG}{every}") },
            HistoryEntry { node: "graph".to_string(), started: 0, ended: 0, outcome: "final".to_string() },
        ];
        let (drawn, changed) = redraw(&mut life, &live(&[]));
        assert!(drawn.is_some() && changed, "a goal again");
        let lifted = life.history.iter().filter(|entry| entry.outcome.starts_with("unban:")).count();
        assert_eq!(lifted, lifescape_planner::TRAINABLE.len(), "one noted expiry per skill");
        let (again, changed) = redraw(&mut life, &live(&[]));
        assert!(again.is_none() && !changed, "an open goal is never redrawn");
    }
}
