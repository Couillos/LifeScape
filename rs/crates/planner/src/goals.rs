//! Goal succession: a life never ends on a finished goal. When a graph
//! finishes (or gets stuck), the brain closes the goal once (history
//! `done:` / `stuck:`, see `bans`) and asks `draw_goal` for the next one,
//! then `append_goal` adds its nodes, climbing from the live levels.
//! Pure: live levels, elapsed ticks and the history come in as arguments.
//!
//! Per-bot ambitions (2026-11-13): `draw_goal_origin` reads the next
//! milestone of `life.ambition` whose skills are not banned; the plain
//! lowest-skill draw (`next_goal`) serves ONLY the explicit `generalist`
//! ambition. A life without ambition draws nothing (typed `NoGoal`): the
//! brain assigns one first, never a silent fallback.

use crate::expand::expand_goal_from;
use crate::model::{GoalKind, HistoryEntry, Life, Node, NodeKind, Profile};

/// Skills the socle trains end to end (gather, fight, cook, fire). The
/// order breaks ties before the seed does.
pub const TRAINABLE: &[&str] = &["woodcutting", "mining", "fishing", "attack", "strength", "defence", "cooking", "firemaking"];

/// History tag of a goal that ended stuck (`node: "goal"`): each skill it
/// trained is banned for `bans::SKILL_BAN_TICKS` (`bans::ban_entry`).
pub const STUCK_TAG: &str = "stuck:";
/// History tag of a goal that ended done.
pub const DONE_TAG: &str = "done:";

/// Skills named by a goal (empty for non-skill goals). A production
/// quota names the skill of its first production leg (its ban unit).
pub fn goal_skills(goal: &GoalKind) -> Vec<String> {
    match goal {
        GoalKind::MaxSkills { skills, .. } => skills.clone(),
        GoalKind::Produce { item, .. } => crate::obtain_legs::production_legs(item)
            .into_iter()
            .find_map(|leg| leg.skill.map(|(skill, _)| skill))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// Why no goal is drawable now (typed, logged by the brain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoGoal {
    /// No ambition yet: the brain assigns one before drawing.
    NoAmbition,
    /// Every open milestone trains a banned skill (retried as bans expire).
    MilestonesBanned(Vec<String>),
    /// Nothing left open: the ambition is achieved (the brain redraws it).
    Achieved,
    /// Generalist: every trainable skill banned or maxed.
    NothingTrainable,
}

impl NoGoal {
    pub fn describe(&self) -> String {
        match self {
            NoGoal::NoAmbition => "no ambition assigned".to_string(),
            NoGoal::MilestonesBanned(skills) => format!("every open milestone trains a banned skill ({})", skills.join(",")),
            NoGoal::Achieved => "ambition achieved, nothing left to draw".to_string(),
            NoGoal::NothingTrainable => "every skill banned or maxed".to_string(),
        }
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

/// THE goal-drawing entry point, called once the current goal is closed:
/// the ambition's next milestone not banned now (generalist: money goes
/// on after a done close, else the lowest trainable skill not banned).
/// `ticks_since` maps a history stamp to elapsed ticks (bans expire).
/// `None`: nothing drawable (see `draw_goal_origin` for the typed
/// reason) — retried every pass, so a ban expiry alone brings the bot back.
pub fn draw_goal(life: &Life, level_of: &dyn Fn(&str) -> u8, ticks_since: &dyn Fn(i64) -> u64) -> Option<GoalKind> {
    draw_goal_origin(life, level_of, ticks_since).ok().map(|(goal, _)| goal)
}

/// `draw_goal` with the milestone the goal comes from (`None` for the
/// generalist) or the typed reason nothing is drawable. Call
/// `ambition::refresh` first (met levels and coins close their milestones).
pub fn draw_goal_origin(life: &Life, level_of: &dyn Fn(&str) -> u8, ticks_since: &dyn Fn(i64) -> u64) -> Result<(GoalKind, Option<String>), NoGoal> {
    let avoid = crate::bans::banned_skills(&life.history, ticks_since);
    let ambition = life.ambition.as_ref().ok_or(NoGoal::NoAmbition)?;
    if ambition.kind != crate::ambition::GENERALIST {
        if let Some(stone) = crate::ambition::next_milestone(ambition, &avoid) {
            return Ok((crate::ambition::goal_for_milestone(stone), Some(stone.id.clone())));
        }
        let open = ambition.milestones.iter().any(|stone| stone.status != crate::ambition::MilestoneStatus::Done);
        return Err(if open { NoGoal::MilestonesBanned(avoid) } else { NoGoal::Achieved });
    }
    generalist_goal(life, level_of, &avoid).map(|goal| (goal, None)).ok_or(NoGoal::NothingTrainable)
}

/// The explicit generalist: money goals go on after a done close, else
/// the lowest trainable skill not banned.
fn generalist_goal(life: &Life, level_of: &dyn Fn(&str) -> u8, avoid: &[String]) -> Option<GoalKind> {
    let closed_done = life
        .history
        .iter()
        .rev()
        .find(|entry| entry.node == "goal" && (entry.outcome.starts_with(DONE_TAG) || entry.outcome.starts_with(STUCK_TAG)))
        .is_some_and(|entry| entry.outcome.starts_with(DONE_TAG));
    if closed_done && matches!(life.goal, GoalKind::MakeMoney | GoalKind::GetRich { .. }) {
        return Some(life.goal.clone());
    }
    next_goal(level_of, avoid, life.seed, life.history.len() as u64)
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

/// True when the current goal's own root node is done: the newest node
/// (highest id: `append_goal` numbers past every older node) of the
/// goal's kind — equip step, quota/obtain, money, quest, area. Older
/// goals' abandoned nodes linger in the graph and keep `graph_state` at
/// `Stuck`; they never make a reached goal a failure (live 2026-11-13,
/// bb4v5d9m: "equip melee" done every 4 s, closed stuck each time). Skill
/// goals answer from the live levels (the brain), never here: `false`.
pub fn goal_root_done(life: &Life) -> bool {
    let serves = |kind: &NodeKind| match (&life.goal, kind) {
        (GoalKind::Obtain { item: goal_item, .. } | GoalKind::Produce { item: goal_item, .. }, NodeKind::ObtainItems { item, .. }) => item == goal_item,
        (GoalKind::MakeMoney | GoalKind::GetRich { .. }, NodeKind::MakeMoney) => true,
        (GoalKind::CompleteQuest { quest: goal_quest }, NodeKind::CompleteQuest { quest }) => quest == goal_quest,
        (GoalKind::EquipBest { style: goal_style }, NodeKind::EquipBest { style }) => style == goal_style,
        (GoalKind::AccessArea { area: goal_area }, NodeKind::AccessArea { area }) => area == goal_area,
        _ => false,
    };
    let number = |node: &Node| node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()).unwrap_or(0);
    life.nodes
        .iter()
        .filter(|node| serves(&node.kind))
        .max_by_key(|node| number(node))
        .is_some_and(|node| node.status == crate::model::NodeStatus::Done)
}

/// Replace the life's goal and append the new goal's nodes, renumbered
/// past every existing id (old nodes stay as the record of past goals).
/// Climbs start at the live level (`level_of`): no tranche already held.
/// A production quota counts from the stock held now (`held_of`: pack,
/// bank, worn): its obtain node asks `held + count`, so stock the bot
/// already had never closes it without producing (review 2026-11-13).
pub fn append_goal(life: &mut Life, goal: GoalKind, profile: &Profile, level_of: &dyn Fn(&str) -> u8, held_of: &dyn Fn(&str) -> i64) {
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
            if let (GoalKind::Produce { item: quota_item, .. }, NodeKind::ObtainItems { item, count }) = (&goal, &mut node.kind) {
                let baseline = held_of(item).max(0);
                if item == quota_item && baseline > 0 {
                    *count = count.saturating_add(baseline);
                    node.progress_target = *count;
                    node.progress_current = baseline;
                }
            }
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
    use crate::model::{Archetype, GoalKind, HistoryEntry, Life, NodeKind, NodeStatus, Profile};

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
            ambition: crate::ambition::new_ambition(
                crate::ambition::GENERALIST,
                "operator",
                &crate::ambition::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 },
                0,
                &[],
                crate::ambition::Stamp { tick: 0, secs: 0 },
            )
            .ok(),
            goal_origin: None,
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
            ambition: None,
            goal_origin: None,
        };
        for node in &mut life.nodes {
            node.status = NodeStatus::Done;
        }
        assert_eq!(graph_state(&life), GraphState::Finished);
        append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 10 }, &profile, &|_| 1, &|_| 0);
        normalize(&mut life);
        assert_eq!(graph_state(&life), GraphState::Launchable);
        let mut ids: Vec<&str> = life.nodes.iter().map(|node| node.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), life.nodes.len(), "ids stay unique");
    }

    /// Review 2026-11-13 (finding 1): a production quota counts what the
    /// bot makes from the draw on, never the stock it already holds (a
    /// cook holding 100 shrimp closed a 50-shrimp quota without cooking).
    #[test]
    fn a_produce_quota_counts_from_the_stock_held_at_draw() {
        let profile = Profile::new(false, 0.5, 0.5, 3, Archetype::Producer);
        let mut life = life_with(GoalKind::MakeMoney, Vec::new());
        let quota = GoalKind::Produce { item: "shrimp".to_string(), count: 50 };
        append_goal(&mut life, quota.clone(), &profile, &|_| 1, &|item| if item == "shrimp" { 100 } else { 0 });
        let node = life.nodes.last().expect("the quota node");
        assert!(matches!(&node.kind, NodeKind::ObtainItems { item, count: 150 } if item == "shrimp"), "{:?}", node.kind);
        assert_eq!((node.progress_current, node.progress_target), (100, 150));
        assert_eq!(life.goal, quota, "the goal keeps its quota");
        append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 10 }, &profile, &|_| 1, &|_| 500);
        assert!(life.nodes.iter().all(|node| !matches!(node.kind, NodeKind::ReachLevel { level, .. } if level != 10)), "other goals untouched");
    }

    /// Live 2026-11-13 (bb4v5d9m, 337 errors/h): "equip melee" ended
    /// `goal_reached` every 4 s, yet an old abandoned obtain (n51, leather
    /// boots, an earlier goal) left the graph Stuck, the goal closed stuck,
    /// its milestone blocked x3 and the ambition flipped warrior <-> priest
    /// every 30 s. The current goal's own root node decides, not old nodes.
    #[test]
    fn a_goal_whose_root_is_done_is_reached_whatever_old_nodes_linger() {
        let profile = Profile::new(false, 0.5, 0.5, 3, Archetype::Producer);
        let mut life = life_with(GoalKind::Obtain { item: "leather_boots".to_string(), count: 1 }, Vec::new());
        append_goal(&mut life, GoalKind::Obtain { item: "leather_boots".to_string(), count: 1 }, &profile, &|_| 1, &|_| 0);
        for node in &mut life.nodes {
            node.status = NodeStatus::Abandoned;
        }
        append_goal(&mut life, GoalKind::EquipBest { style: "melee".to_string() }, &profile, &|_| 1, &|_| 0);
        assert!(!super::goal_root_done(&life), "the equip step has not run yet");
        let last = life.nodes.len() - 1;
        life.nodes[last].status = NodeStatus::Done;
        assert!(matches!(graph_state(&life), GraphState::Stuck { .. }), "the old abandon still marks the whole graph");
        assert!(super::goal_root_done(&life), "the goal's own step is done");
        // An older done root of the same kind never answers for a newer one.
        append_goal(&mut life, GoalKind::EquipBest { style: "melee".to_string() }, &profile, &|_| 1, &|_| 0);
        assert!(!super::goal_root_done(&life), "the newest equip step decides");
        // A quota abandoned by cascade is not reached; done is.
        append_goal(&mut life, GoalKind::Produce { item: "shrimp".to_string(), count: 5 }, &profile, &|_| 1, &|_| 0);
        let last = life.nodes.len() - 1;
        life.nodes[last].status = NodeStatus::Abandoned;
        assert!(!super::goal_root_done(&life));
        life.nodes[last].status = NodeStatus::Done;
        assert!(super::goal_root_done(&life));
        // Skill goals answer from the live levels, never from a node.
        append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 10 }, &profile, &|_| 1, &|_| 0);
        for node in &mut life.nodes {
            node.status = NodeStatus::Done;
        }
        assert!(!super::goal_root_done(&life));
    }
}
