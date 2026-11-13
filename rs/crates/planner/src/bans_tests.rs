//! Tests of `bans.rs` (temporary skill bans, user decision 2026-11-13).

use super::{ban_entry, ban_states, banned_skills, stuck_cause, unban_entry, BanState, StuckCause, SKILL_BAN_TICKS};
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

/// Elapsed ticks with 1 s = 1 tick: the brain maps wall time itself.
fn since(now: i64) -> impl Fn(i64) -> u64 {
    move |at| (now - at).max(0) as u64
}

fn cause() -> StuckCause {
    StuckCause { node: "n42".to_string(), class: "3 failures".to_string(), last_error: "Failed: log did not catch fire".to_string() }
}

#[test]
fn a_ban_lasts_its_ticks_then_expires() {
    let history = vec![entry("goal", &ban_entry("firemaking", SKILL_BAN_TICKS, &cause()), 100)];
    assert_eq!(banned_skills(&history, &since(100 + 5999)), vec!["firemaking".to_string()], "one tick short: still banned");
    assert!(banned_skills(&history, &since(100 + 6000)).is_empty(), "6000 ticks later: drawable again");
    match &ban_states(&history, &since(100 + 10))[0] {
        (skill, BanState::Active { ticks_left, cause }) => {
            assert_eq!(skill, "firemaking");
            assert_eq!(*ticks_left, SKILL_BAN_TICKS - 10);
            assert!(cause.contains("3 failures") && cause.contains("n42") && cause.contains("log did not catch fire"), "cause kept: {cause}");
        }
        other => panic!("active ban, got {other:?}"),
    }
}

/// The 12 idle bots of 2026-11-13: every skill banned for life by the
/// old `stuck:<skills>` entries. Without a tick they count as expired.
#[test]
fn legacy_entries_count_as_expired() {
    let history = vec![
        entry("goal", "stuck:fishing,cooking", 10),
        entry("goal", "stuck:strength", 20),
        entry("goal", "stuck:strength", 30),
    ];
    assert!(banned_skills(&history, &since(31)).is_empty(), "no tick, no ban");
    let states = ban_states(&history, &since(31));
    let skills: Vec<&str> = states.iter().map(|(skill, _)| skill.as_str()).collect();
    assert_eq!(skills, vec!["cooking", "fishing", "strength"], "one state per skill, sorted");
    assert!(states.iter().all(|(_, state)| matches!(state, BanState::Expired { legacy: true, .. })));
}

/// An expiry is noted once (`unban:<skill>`): the brain logs it, then
/// the skill reads lifted until a new ban.
#[test]
fn an_unban_entry_lifts_until_the_next_ban() {
    let mut history = vec![entry("goal", &ban_entry("mining", SKILL_BAN_TICKS, &cause()), 0)];
    assert!(matches!(ban_states(&history, &since(7000))[0].1, BanState::Expired { legacy: false, .. }));
    history.push(entry("goal", &unban_entry("mining"), 7000));
    assert!(ban_states(&history, &since(7001)).is_empty(), "lifted skills drop out");
    history.push(entry("goal", &ban_entry("mining", SKILL_BAN_TICKS, &cause()), 8000));
    assert_eq!(banned_skills(&history, &since(8001)), vec!["mining".to_string()], "a new failure bans again");
}

#[test]
fn ban_entries_ignore_other_history() {
    let history = vec![entry("n1", "stuck:fishing@6000:x", 0), entry("goal", "done:fishing", 0), entry("goal", "stuck:@0:no skill", 0)];
    assert!(ban_states(&history, &since(1)).is_empty(), "node entries, done goals and skill-less closes ban nothing");
}

fn node(id: &str, status: NodeStatus, failures: u32, blocks: u32, error: &str) -> Node {
    let mut node = Node::new(id, NodeKind::ReachLevel { skill: "firemaking".to_string(), level: 15 }, Vec::new(), 15);
    node.status = status;
    node.failures = failures;
    node.blocks = blocks;
    node.last_error = error.to_string();
    node
}

fn life_of(nodes: Vec<Node>, history: Vec<HistoryEntry>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["firemaking".to_string()], level: 15 },
        nodes,
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: None,
    }
}

/// The cause is the root of the latest abandonment: a cascade line
/// (`dependency nX abandoned`) is followed to the node that really failed.
#[test]
fn the_stuck_cause_is_the_root_of_the_latest_abandon() {
    let life = life_of(
        vec![
            node("n1", NodeStatus::Abandoned, 3, 0, "Failed: old trouble"),
            node("n2", NodeStatus::Abandoned, 3, 0, "Failed: log did not catch fire"),
            node("n3", NodeStatus::Abandoned, 0, 0, "dependency n2 abandoned"),
        ],
        vec![entry("n1", "abandoned", 1), entry("n2", "abandoned", 2), entry("graph", "stuck", 3)],
    );
    let found = stuck_cause(&life).expect("a cause");
    assert_eq!(found.node, "n2");
    assert_eq!(found.class, "3 failures");
    assert_eq!(found.last_error, "Failed: log did not catch fire");
    let cascaded = life_of(
        vec![node("n2", NodeStatus::Abandoned, 0, 4, "Blocked: missing tinderbox"), node("n3", NodeStatus::Abandoned, 0, 0, "dependency n2 abandoned")],
        Vec::new(),
    );
    let found = stuck_cause(&cascaded).expect("graph fallback");
    assert_eq!((found.node.as_str(), found.class.as_str()), ("n2", "4 blocks"), "no history: first root in the graph");
    let contract = life_of(vec![node("n5", NodeStatus::Abandoned, 1, 0, "Usage: bad spot")], Vec::new());
    assert_eq!(stuck_cause(&contract).map(|found| found.class), Some("contract error".to_string()));
    assert_eq!(stuck_cause(&life_of(Vec::new(), Vec::new())), None);
}
