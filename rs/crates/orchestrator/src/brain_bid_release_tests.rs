//! lily_north, live 2026-11-20 22:22 -> 23:22 UTC: priest ambition,
//! `bid:mithril_ore` 93 units, n1401 waiting on `full:farm:mithril_ore`
//! the whole hour (fill-n1401 in a loop). Past one cooldown of waiting,
//! the answer is given back: its work abandoned, no ban, its claims
//! freed, the next goal drawn from the priest's milestones.

use lifescape_json::{object, Value};
use lifescape_planner::{new_ambition, Archetype, BotNow, GoalKind, HistoryEntry, Life, MilestoneStatus, Node, NodeKind, NodeStatus, Profile, Stamp};

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Every skill at 30.
fn live() -> Value {
    let skill = |id: i64| object(vec![("id", Value::Int(id)), ("level", Value::Int(30))]);
    object(vec![("skills", Value::Arr((0..=20).map(skill).collect()))])
}

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

/// The priest answering the mithril bid, n1401 waiting since `first_wait`
/// (unix seconds), the method cooled again 10 s ago (the re-read after
/// the lapse found every site still full).
fn lily(account: &str, first_wait: i64) -> Life {
    let now = now_secs();
    let held = new_ambition("priest", "need", &BotNow { level_of: &|_| 30, coins: 0, held: &|_| 0 }, 7, &[], Stamp { tick: 0, secs: now - 400_000 }).expect("a priest");
    let mut ore = Node::new("n1401", NodeKind::ObtainItems { item: "mithril_ore".to_string(), count: 93 }, Vec::new(), 93);
    ore.status = NodeStatus::Ready;
    ore.method = Some("farm:mithril_ore".to_string());
    let mut life = Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::Produce { item: "mithril_ore".to_string(), count: 93 },
        nodes: vec![ore],
        engagement: None,
        active: None,
        history: vec![
            entry("goal", "next:produce mithril_ore 93 (bid:mithril_ore)", first_wait - 1),
            entry("n1401", "cool_wait:farm:mithril_ore", first_wait),
            entry("cool", "cool:farm:mithril_ore|6000|full:farm:mithril_ore", now - 10),
        ],
        ambition: Some(held),
        goal_origin: Some("bid:mithril_ore".to_string()),
        memory: Default::default(),
    };
    life.memory.floor = Some(1401);
    life
}

/// Past the delay: given back, the priest's milestone drawn, nothing
/// banned, no milestone blocked, the lot's claim and purchases freed.
#[test]
fn a_bid_answer_blocked_on_full_sites_is_given_back_to_the_ambition() {
    let account = "release_lily_north";
    let mut life = lily(account, now_secs() - 100_000);
    // The draw refreshes progress and activates a milestone: compare
    // what a block would change (ids, targets, block counts).
    let stones = |life: &Life| life.ambition.as_ref().map(|held| held.milestones.iter().map(|stone| (stone.id.clone(), stone.target.clone(), stone.blocked_count)).collect::<Vec<_>>());
    let stones_before = stones(&life);
    crate::market_claims::claim(account, "release_mithril_ore", 93, 3_600);
    crate::market_claims::claim_purchases(account, &[("release_pickaxe".to_string(), 1)], 3_600);
    assert!(super::release_blocked_bid(&mut life, &live()), "the answer is given back");
    let status = life.nodes.iter().find(|node| node.id == "n1401").map(|node| node.status.clone());
    assert_eq!(status, Some(NodeStatus::Abandoned), "its work stops");
    let origin = life.goal_origin.clone().expect("a new goal");
    assert!(life.ambition.as_ref().is_some_and(|held| held.milestones.iter().any(|stone| stone.id == origin)), "back on the priest's milestones: {origin}");
    assert_eq!(life.ambition.as_ref().map(|held| held.kind.as_str()), Some("priest"), "ambition untouched");
    assert_eq!(stones(&life), stones_before, "no milestone blocked");
    assert!(life.ambition.as_ref().is_some_and(|held| held.milestones.iter().all(|stone| stone.status != MilestoneStatus::Blocked)));
    assert!(!life.history.iter().any(|entry| entry.outcome.starts_with("ban:") || entry.outcome.starts_with("stuck:")), "nothing banned: {:?}", life.history);
    assert!(life.history.iter().any(|entry| entry.outcome.starts_with("bid_released:bid:mithril_ore")), "the give-back is recorded");
    assert_eq!(crate::market_claims::claimed_by_others("someone_else").get("release_mithril_ore"), None, "the lot's claim is freed");
    assert_eq!(crate::market_claims::purchases_by_others("someone_else").get("release_pickaxe"), None, "its purchases are freed");
    assert!(!super::release_blocked_bid(&mut life, &live()), "once");
}

/// Under the delay, while a node runs, or for a milestone goal: kept.
#[test]
fn a_short_wait_a_running_node_or_a_milestone_goal_keep_the_answer() {
    let mut fresh = lily("release_fresh", now_secs() - 5);
    assert!(!super::release_blocked_bid(&mut fresh, &live()), "under one cooldown");
    assert_eq!(fresh.goal_origin.as_deref(), Some("bid:mithril_ore"));
    let mut running = lily("release_running", now_secs() - 100_000);
    running.active = Some("n1400".to_string());
    assert!(!super::release_blocked_bid(&mut running, &live()), "a run's report decides first");
    let mut milestone = lily("release_milestone", now_secs() - 100_000);
    milestone.goal_origin = milestone.ambition.as_ref().and_then(|held| held.milestones.first()).map(|stone| stone.id.clone());
    assert!(!super::release_blocked_bid(&mut milestone, &live()), "a milestone goal waits for its ambition");
}

/// Review point 3: a daemon restart forgets the engaged seats; the first
/// pass re-engages an open bid goal's seats, once. Point 6: freeing the
/// board (role switch, operator stop) ends claims, purchases and seats,
/// and lets a later pass re-engage.
#[test]
fn a_restart_re_engages_and_a_freed_board_holds_nothing() {
    let account = "reengage_miner";
    let mut life = lily(account, now_secs() - 5);
    life.history.retain(|entry| entry.node == "goal");
    assert!(!crate::site_crowd::is_engaged(account), "a fresh daemon knows no seat");
    // The draw's make path for the lot (`brain_bids::lot_seats`, live
    // rates and book): the Lumbridge swamp mithril field.
    let swamp = |item: &str, _count: i64| if item == "mithril_ore" { vec![("farm:mithril_ore".to_string(), (3243, 3167, 0))] } else { Vec::new() };
    assert!(super::reengage_with(&life, &swamp), "the open mithril answer holds a seat again");
    assert!(crate::site_crowd::is_engaged(account));
    let others = crate::site_crowd::seats("reengage_other");
    assert!(others.iter().any(|(_, method, _)| method == "farm:mithril_ore"), "the others read it taken: {others:?}");
    assert!(!super::reengage_with(&life, &swamp), "once");
    crate::market_claims::claim(account, "reengage_mithril", 93, 3_600);
    crate::market_claims::claim_purchases(account, &[("reengage_pickaxe".to_string(), 1)], 3_600);
    super::free_board(account);
    assert!(!crate::site_crowd::is_engaged(account), "seats freed");
    assert_eq!(crate::market_claims::claimed_by_others("reengage_other").get("reengage_mithril"), None, "claim freed");
    assert_eq!(crate::market_claims::purchases_by_others("reengage_other").get("reengage_pickaxe"), None, "purchases freed");
    let mut milestone = life.clone();
    milestone.goal_origin = Some("m1".to_string());
    assert!(!super::reengage_with(&milestone, &swamp), "only a bid goal holds seats");
}

/// `bid_trade` on the persisted state (live 2026-11-20: 60 of 63 bots
/// on answers drawn before the rule): the priest's mithril ore answer is
/// off its trade, given back at once (no wait), no ban, back on the
/// priest's milestones; a running node decides first; a miner keeps it.
#[test]
fn an_answer_off_the_trade_is_given_back_to_the_ambition() {
    let account = "offtrade_lily_north";
    let mut life = lily(account, now_secs() - 5);
    crate::market_claims::claim(account, "offtrade_mithril_ore", 93, 3_600);
    let mut running = life.clone();
    running.active = Some("n1401".to_string());
    assert!(!super::release_off_trade(&mut running, &live()), "a run's report decides first");
    assert!(super::release_off_trade(&mut life, &live()), "off the priest's trade");
    let status = life.nodes.iter().find(|node| node.id == "n1401").map(|node| node.status.clone());
    assert_eq!(status, Some(NodeStatus::Abandoned), "its work stops");
    let origin = life.goal_origin.clone().expect("a new goal");
    assert!(life.ambition.as_ref().is_some_and(|held| held.milestones.iter().any(|stone| stone.id == origin)), "back on the priest's milestones: {origin}");
    assert!(!life.history.iter().any(|entry| entry.outcome.starts_with("ban:") || entry.outcome.starts_with("stuck:")), "nothing banned: {:?}", life.history);
    assert!(life.history.iter().any(|entry| entry.outcome == format!("{}bid:mithril_ore", super::OFF_TRADE_TAG)), "recorded");
    assert_eq!(crate::market_claims::claimed_by_others("someone_else").get("offtrade_mithril_ore"), None, "the lot's claim is freed");
    assert!(!super::release_off_trade(&mut life, &live()), "once");
    let mut miner = lily("offtrade_miner", now_secs() - 5);
    miner.ambition = new_ambition("miner", "need", &BotNow { level_of: &|_| 30, coins: 0, held: &|_| 0 }, 7, &[], Stamp { tick: 0, secs: now_secs() - 400_000 }).ok();
    assert!(!super::release_off_trade(&mut miner, &live()), "ore is the miner's trade");
    assert_eq!(miner.goal_origin.as_deref(), Some("bid:mithril_ore"));
}
