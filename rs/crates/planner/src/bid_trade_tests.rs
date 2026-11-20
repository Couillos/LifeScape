//! Live 2026-11-20 00:15 -> 01:15 UTC: 60 of the 63 bots outside the
//! market makers held a `bid:` goal; nina_87 (runecrafter) made mithril
//! bars, lily_north (priest) mithril battleaxes, james_mason (cook)
//! water runes; cooking, prayer and firemaking 0 % of the fleet's time.

use super::{answers_trade, drawn_while_waiting, off_trade_answer, WAIT_ANSWER_TAG};
use crate::ambition::Ambition;
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Profile};

fn ambition(kind: &str) -> Ambition {
    Ambition {
        kind: kind.to_string(),
        label: kind.to_string(),
        groups: Vec::new(),
        why: "random".to_string(),
        milestones: Vec::new(),
        drawn_at_tick: 0,
        drawn_at: 0,
        rung: 0,
        revisions: Vec::new(),
        pin: None,
        draw_rule: "need".to_string(),
    }
}

fn life(account: &str, kind: Option<&str>, origin: Option<&str>) -> Life {
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal: GoalKind::Produce { item: "mithril_bar".to_string(), count: 53 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: kind.map(ambition),
        goal_origin: origin.map(str::to_string),
        memory: Default::default(),
    }
}

#[test]
fn a_kind_answers_what_its_primary_skill_makes() {
    assert!(answers_trade(&life("eva_dale", Some("smith"), None), "mithril_platebody"));
    assert!(answers_trade(&life("eva_dale", Some("smith"), None), "mithril_bar"));
    assert!(answers_trade(&life("nina_87", Some("runecrafter"), None), "waterrune"));
    assert!(answers_trade(&life("james_mason", Some("cook"), None), "lobster"));
    assert!(answers_trade(&life("paul_rhodes", Some("miner"), None), "coal"));
    assert!(answers_trade(&life("tom_king", Some("crafter"), None), "sapphire_necklace"));
}

#[test]
fn off_its_trade_a_need_kind_answers_nothing() {
    // The secondary group (runecrafter: mining for essence) is no trade
    // to sell: mithril bars are a smith's.
    assert!(!answers_trade(&life("nina_87", Some("runecrafter"), None), "mithril_bar"));
    assert!(!answers_trade(&life("lily_north", Some("priest"), None), "mithril_battleaxe"));
    assert!(!answers_trade(&life("james_mason", Some("cook"), None), "waterrune"));
    // A miner sells ore, not the bars made of it.
    assert!(!answers_trade(&life("paul_rhodes", Some("miner"), None), "mithril_bar"));
    // The consumers make nothing: they train and buy what they burn.
    for kind in ["priest", "firemaker", "ranger", "warrior"] {
        assert!(!answers_trade(&life("someone", Some(kind), None), "steel_platebody"), "{kind}");
    }
}

#[test]
fn the_exceptions_answer_the_whole_market() {
    // D3: Zezima, N0valyfe, Yogosun keep the level-led draw and the
    // whole market; kinds without a trade (market maker, overall,
    // generalist, alchemist) and lives without ambition too.
    assert!(answers_trade(&life("Zezima", Some("runecrafter"), None), "mithril_bar"));
    assert!(answers_trade(&life("n0valyfe", Some("priest"), None), "mithril_bar"));
    for kind in ["market_maker", "overall", "generalist", "alcher"] {
        assert!(answers_trade(&life("someone", Some(kind), None), "mithril_bar"), "{kind}");
    }
    assert!(answers_trade(&life("someone", None, None), "mithril_bar"));
}

#[test]
fn a_held_answer_off_the_trade_is_named() {
    let nina = life("nina_87", Some("runecrafter"), Some("bid:mithril_bar"));
    assert_eq!(off_trade_answer(&nina), Some("bid:mithril_bar".to_string()));
    let smith = life("eva_dale", Some("smith"), Some("bid:mithril_bar"));
    assert_eq!(off_trade_answer(&smith), None);
    let milestone = life("nina_87", Some("runecrafter"), Some("m1"));
    assert_eq!(off_trade_answer(&milestone), None);
    // A goal already closed is the draw's to replace.
    let mut closed = life("nina_87", Some("runecrafter"), Some("bid:mithril_bar"));
    closed.history.push(HistoryEntry { node: "goal".to_string(), started: 1, ended: 1, outcome: format!("{}produce mithril_bar 53", crate::goals::DONE_TAG) });
    assert!(crate::goals::goal_closed(&closed.history));
    assert_eq!(off_trade_answer(&closed), None);
}

/// The hybrid (2026-11-20): an answer drawn while the ambition waits on
/// a buy is of any trade and stays; the next draw ends the mark.
#[test]
fn an_answer_drawn_while_waiting_is_kept_off_the_trade() {
    let goal = |outcome: &str| HistoryEntry { node: "goal".to_string(), started: 1, ended: 1, outcome: outcome.to_string() };
    let next = format!("{}produce mithril_bar 53 (bid:mithril_bar)", crate::ambition_review::NEXT_TAG);
    let mut nina = life("nina_87", Some("runecrafter"), Some("bid:mithril_bar"));
    nina.history = vec![goal(&next), goal(&format!("{WAIT_ANSWER_TAG}bid:mithril_bar"))];
    assert!(drawn_while_waiting(&nina.history));
    assert_eq!(off_trade_answer(&nina), None, "kept while waiting");
    // A later draw (the same object off the trade, drawn normally): the
    // mark belongs to the earlier goal.
    nina.history.push(goal(&next));
    assert!(!drawn_while_waiting(&nina.history));
    assert_eq!(off_trade_answer(&nina), Some("bid:mithril_bar".to_string()));
}

/// Review 2026-11-20: a production milestone set aside keeps what its
/// goal made since the draw (not the stock it held then); the redraw
/// asks the rest only.
#[test]
fn a_production_milestone_set_aside_keeps_what_it_made() {
    use crate::ambition::{Milestone, MilestoneStatus, Target};
    use crate::model::{Node, NodeKind};
    let mut cook = life("mila_wade", Some("cook"), Some("m2"));
    let stone = Milestone {
        id: "m2".to_string(),
        label: "produire 170 apple_pie".to_string(),
        target: Target::Produce { item: "apple_pie".to_string(), count: 170, skill: "cooking".to_string() },
        status: MilestoneStatus::Active,
        progress: (0, 170),
        blocked_count: 0,
        origin: "ladder".to_string(),
    };
    cook.ambition.as_mut().expect("cook").milestones.push(stone);
    cook.goal = GoalKind::Produce { item: "apple_pie".to_string(), count: 170 };
    cook.memory.floor = Some(2000);
    // 12 pies held at the draw (target 182, start 12), 40 made since.
    let mut quota = Node::new("n2000", NodeKind::ObtainItems { item: "apple_pie".to_string(), count: 182 }, Vec::new(), 182);
    quota.progress_current = 52;
    // An earlier goal's pie node (under the floor) counts nothing.
    let mut old = Node::new("n1500", NodeKind::ObtainItems { item: "apple_pie".to_string(), count: 500 }, Vec::new(), 500);
    old.progress_current = 400;
    cook.nodes = vec![old, quota];
    assert_eq!(super::credit_made(&mut cook, "m2"), 40);
    let held = &cook.ambition.as_ref().expect("cook").milestones[0];
    assert_eq!(held.progress, (40, 170));
    assert_eq!(held.status, MilestoneStatus::Active, "not done");
    assert_eq!(crate::ambition::goal_for_milestone(held), GoalKind::Produce { item: "apple_pie".to_string(), count: 130 });
    // A level goal credits nothing.
    let mut priest = life("lily_north", Some("priest"), Some("m1"));
    priest.goal = GoalKind::MaxSkills { skills: vec!["prayer".to_string()], level: 70 };
    assert_eq!(super::credit_made(&mut priest, "m1"), 0);
}

/// Review 2026-11-20: a buy left on the book by an earlier goal (set
/// aside, given back) keeps no later goal open: the answer's graph
/// finishes, the ambition is drawn back. The current goal's own wait
/// keeps it launchable (the filler or the hybrid).
#[test]
fn a_buy_of_an_earlier_goal_keeps_no_graph_open() {
    use crate::lifecycle::{graph_state, GraphState};
    use crate::model::{Node, NodeKind, NodeStatus};
    let parked = |id: &str| {
        let mut bones = Node::new(id, NodeKind::ObtainItems { item: "big_bones".to_string(), count: 500 }, Vec::new(), 500);
        bones.status = NodeStatus::Ready;
        bones.method = Some("market:buy".to_string());
        bones
    };
    let mut done = Node::new("n1500", NodeKind::ObtainItems { item: "steel_platebody".to_string(), count: 20 }, Vec::new(), 20);
    done.status = NodeStatus::Done;
    let mut lily = life("lily_north", Some("priest"), Some("bid:steel_platebody"));
    lily.memory.floor = Some(1500);
    lily.nodes = vec![parked("n1433"), done.clone()];
    lily.history = vec![HistoryEntry { node: "n1433".to_string(), started: 1, ended: 1, outcome: "awaiting_fill".to_string() }];
    assert_eq!(graph_state(&lily), GraphState::Finished);
    // The same wait, the current goal's own: launchable.
    lily.nodes = vec![parked("n1501"), done];
    lily.history = vec![HistoryEntry { node: "n1501".to_string(), started: 1, ended: 1, outcome: "awaiting_fill".to_string() }];
    assert_eq!(graph_state(&lily), GraphState::Launchable);
}
