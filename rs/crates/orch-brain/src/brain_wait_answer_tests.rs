//! Live 2026-11-20 01:36 -> 02:36 UTC: lily_north (priest, prayer 70)
//! bid for big bones nobody sold and filled her wait with willow logs;
//! fillers 37 % of the fleet's time. The hybrid sets her milestone aside
//! for a market answer of any trade, her bones bid left on the book.

use lifescape_json::Value;
use lifescape_planner::{new_ambition, Archetype, BotNow, GoalKind, HistoryEntry, Life, Milestone, MilestoneStatus, Node, NodeKind, NodeStatus, Profile, Stamp, Target};

/// History stamps are plain ticks here (`ticks_since` = NOW - stamp).
const NOW: i64 = 1_000_000;

fn ticks_since(stamp: i64) -> u64 {
    (NOW - stamp).max(0) as u64
}

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

/// The priest on her first milestone, its bones bid parked since
/// `parked_at`, a ready bury node behind it.
fn lily(parked_at: i64) -> Life {
    let held = new_ambition("priest", "need", &BotNow { level_of: &|_| 30, coins: 0, held: &|_| 0 }, 7, &[], Stamp { tick: 0, secs: 0 }).expect("a priest");
    let first = held.milestones.first().map(|stone| stone.id.clone()).expect("a milestone");
    let mut bones = Node::new("n1433", NodeKind::ObtainItems { item: "big_bones".to_string(), count: 500 }, Vec::new(), 500);
    bones.status = NodeStatus::Ready;
    bones.method = Some("market:buy".to_string());
    let mut bury = Node::new("n1434", NodeKind::ObtainItems { item: "big_bones".to_string(), count: 1 }, Vec::new(), 1);
    bury.status = NodeStatus::Ready;
    let mut life = Life {
        schema: 1,
        account: "wait_lily_north".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["prayer".to_string()], level: 70 },
        nodes: vec![bones, bury],
        engagement: None,
        active: None,
        history: vec![entry("goal", "next:prayer 70", parked_at - 10), entry("n1433", "awaiting_fill", parked_at)],
        ambition: Some(held),
        goal_origin: Some(first),
        memory: Default::default(),
    };
    life.memory.floor = Some(1433);
    life
}

/// The draw of a whole-market answer (what `draw_next_with(.., true)`
/// appends after the real close: a production quota, origin `bid:`).
fn draw_answer(life: &mut Life) -> Option<String> {
    life.goal = GoalKind::Produce { item: "steel_platebody".to_string(), count: 20 };
    life.goal_origin = Some("bid:steel_platebody".to_string());
    life.history.push(entry("goal", "next:produce steel_platebody 20 (bid:steel_platebody)", NOW));
    Some("produce steel_platebody 20".to_string())
}

#[test]
fn a_milestone_waiting_on_a_buy_is_set_aside_for_an_answer_of_any_trade() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut life = lily(NOW - 2_000);
    let stones_before = life.ambition.as_ref().map(|held| held.milestones.iter().map(|stone| (stone.id.clone(), stone.status.clone(), stone.progress, stone.blocked_count)).collect::<Vec<_>>());
    assert!(super::set_aside_with(&mut life, &Value::Null, &ticks_since, &|_| true, &mut draw_answer, deps));
    assert_eq!(life.goal_origin.as_deref(), Some("bid:steel_platebody"));
    let stones_after = life.ambition.as_ref().map(|held| held.milestones.iter().map(|stone| (stone.id.clone(), stone.status.clone(), stone.progress, stone.blocked_count)).collect::<Vec<_>>());
    assert_eq!(stones_after, stones_before, "the milestone is neither done nor blocked");
    let status = |id: &str| life.nodes.iter().find(|node| node.id == id).map(|node| node.status.clone());
    assert_eq!(status("n1433"), Some(NodeStatus::Ready), "the posted bones bid stays on the book");
    assert_eq!(status("n1434"), Some(NodeStatus::Abandoned), "the rest of the goal's work stops");
    assert!(lifescape_planner::bid_trade::drawn_while_waiting(&life.history), "marked");
    assert_eq!(lifescape_planner::bid_trade::off_trade_answer(&life), None, "a smithing lot is no priest's trade, kept while waiting");
}

#[test]
fn a_short_wait_a_running_node_a_non_milestone_goal_or_no_lot_keep_the_goal() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut never = |_: &mut Life| -> Option<String> { panic!("no draw expected") };
    // Under one filler window.
    let mut fresh = lily(NOW - 500);
    assert!(!super::set_aside_with(&mut fresh, &Value::Null, &ticks_since, &|_| true, &mut never, deps));
    // A node runs: its report first.
    let mut running = lily(NOW - 2_000);
    running.active = Some("n1434".to_string());
    assert!(!super::set_aside_with(&mut running, &Value::Null, &ticks_since, &|_| true, &mut never, deps));
    // A bid answer already (its own waits use the filler).
    let mut answering = lily(NOW - 2_000);
    answering.goal_origin = Some("bid:big_bones".to_string());
    assert!(!super::set_aside_with(&mut answering, &Value::Null, &ticks_since, &|_| true, &mut never, deps));
    // No lot pays now: the filler runs as before, nothing touched.
    let mut idle = lily(NOW - 2_000);
    let before = idle.clone();
    assert!(!super::set_aside_with(&mut idle, &Value::Null, &ticks_since, &|_| false, &mut never, deps));
    assert_eq!(idle, before);
}

#[test]
fn a_draw_that_returns_to_the_milestone_is_not_marked() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut life = lily(NOW - 2_000);
    let first = life.goal_origin.clone();
    let mut back = |life: &mut Life| {
        life.goal_origin = first.clone();
        life.history.push(entry("goal", "next:prayer 70", NOW));
        Some("prayer 70".to_string())
    };
    assert!(super::set_aside_with(&mut life, &Value::Null, &ticks_since, &|_| true, &mut back, deps));
    assert!(!lifescape_planner::bid_trade::drawn_while_waiting(&life.history));
}

/// A production milestone (the close would mark it DONE if it were still
/// linked: `on_goal_closed` on `Target::Produce`): set aside, it stays
/// open, credited with what its goal made, and asks the rest.
#[test]
fn a_production_milestone_set_aside_is_neither_done_nor_lost() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut cook = lily(NOW - 2_000);
    cook.account = "wait_mila_wade".to_string();
    let stone = Milestone {
        id: "m42".to_string(),
        label: "produire 170 apple_pie".to_string(),
        target: Target::Produce { item: "apple_pie".to_string(), count: 170, skill: "cooking".to_string() },
        status: MilestoneStatus::Active,
        progress: (0, 170),
        blocked_count: 0,
        origin: "ladder".to_string(),
    };
    cook.ambition.as_mut().expect("ambition").milestones.push(stone);
    cook.goal_origin = Some("m42".to_string());
    cook.goal = GoalKind::Produce { item: "apple_pie".to_string(), count: 170 };
    let mut pies = Node::new("n1435", NodeKind::ObtainItems { item: "apple_pie".to_string(), count: 170 }, Vec::new(), 170);
    pies.status = NodeStatus::Ready;
    pies.progress_current = 25;
    cook.nodes.push(pies);
    assert!(super::set_aside_with(&mut cook, &Value::Null, &ticks_since, &|_| true, &mut draw_answer, deps));
    let held = cook.ambition.as_ref().and_then(|ambition| ambition.milestones.iter().find(|stone| stone.id == "m42")).cloned().expect("m42 kept");
    assert_ne!(held.status, MilestoneStatus::Done, "not done");
    assert_eq!(held.blocked_count, 0, "not blocked");
    assert_eq!(held.progress, (25, 170), "what it made is credited");
    assert_eq!(lifescape_planner::goal_for_milestone(&held), GoalKind::Produce { item: "apple_pie".to_string(), count: 145 });
}

/// Review 2026-11-20: a buy left by an earlier goal (under the floor)
/// never sets the next goal aside.
#[test]
fn a_buy_of_an_earlier_goal_sets_nothing_aside() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let never = &mut |_: &mut Life| -> Option<String> { panic!("no draw expected") };
    let mut life = lily(NOW - 2_000);
    life.memory.floor = Some(1500);
    assert!(!super::set_aside_with(&mut life, &Value::Null, &ticks_since, &|_| true, never, deps));
}

/// Live 2026-11-20 08:43 -> 09:43 UTC: 13 miners on gold and adamantite
/// rocks, every site full; each waited the cooldown (6000 ticks) on a
/// filler, fillers 30 % of the fleet's time. The rocks taken, the goal is
/// set aside at once for a market answer (the cooldown outlasts a filler
/// window); a cool run out or an earlier goal's wait sets nothing aside.
fn miner(cooled_at: i64) -> Life {
    use lifescape_planner::cooldown::{cool_entry, COOL_NODE, COOL_TICKS, WAIT_TAG};
    let mut life = lily(NOW);
    life.account = "wait_harry_page".to_string();
    life.goal = GoalKind::Produce { item: "gold_ore".to_string(), count: 100 };
    let mut ore = Node::new("n1500", NodeKind::ObtainItems { item: "gold_ore".to_string(), count: 100 }, Vec::new(), 100);
    ore.status = NodeStatus::Ready;
    life.nodes = vec![ore];
    life.memory.floor = Some(1500);
    life.history = vec![
        entry("goal", "next:produce gold_ore 100", cooled_at - 10),
        entry(COOL_NODE, &cool_entry("farm:gold_ore", COOL_TICKS, "full:farm:gold_ore"), cooled_at),
        entry("n1500", &format!("{WAIT_TAG}farm:gold_ore"), cooled_at),
    ];
    life
}

#[test]
fn a_goal_whose_every_site_is_full_is_set_aside_at_once() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut life = miner(NOW - 10);
    assert!(super::set_aside_with(&mut life, &Value::Null, &ticks_since, &|_| true, &mut draw_answer, deps));
    assert_eq!(life.goal_origin.as_deref(), Some("bid:steel_platebody"));
    assert!(lifescape_planner::bid_trade::drawn_while_waiting(&life.history));
    // A side goal (its milestones banned) on full rocks: the same.
    let mut side = miner(NOW - 10);
    side.goal_origin = Some(format!("{}demand:gold_ore", lifescape_planner::SIDE_PREFIX));
    assert!(super::set_aside_with(&mut side, &Value::Null, &ticks_since, &|_| true, &mut draw_answer, deps));
}

#[test]
fn a_cool_run_out_or_an_earlier_goals_wait_sets_nothing_aside() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let never = &mut |_: &mut Life| -> Option<String> { panic!("no draw expected") };
    let mut over = miner(NOW - 7_000);
    assert!(!super::set_aside_with(&mut over, &Value::Null, &ticks_since, &|_| true, never, deps));
    let mut earlier = miner(NOW - 10);
    earlier.memory.floor = Some(1600);
    assert!(!super::set_aside_with(&mut earlier, &Value::Null, &ticks_since, &|_| true, never, deps));
}

/// A wait spent with the daemon stopped does not set the milestone aside
/// (user 2026-11-23); the same wait awake does.
#[test]
fn a_wait_spent_with_the_daemon_down_sets_nothing_aside() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let parked_at = NOW - 2_000;
    let mut awake = lily(parked_at);
    assert!(super::set_aside_with(&mut awake, &Value::Null, &ticks_since, &|_| true, &mut draw_answer, deps), "control: 2 000 ticks awake");
    lifescape_orch_core::downtime::set_for_test(vec![(parked_at + 10, parked_at + 1_900)]);
    let mut stopped = lily(parked_at);
    let mut never = |_: &mut Life| -> Option<String> { panic!("no draw expected") };
    assert!(!super::set_aside_with(&mut stopped, &Value::Null, &ticks_since, &|_| true, &mut never, deps), "110 ticks awake");
    lifescape_orch_core::downtime::set_for_test(Vec::new());
}
