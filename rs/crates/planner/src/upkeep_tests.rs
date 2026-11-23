//! The life upkeep decisions (lot 4), without a daemon: a life, typed
//! facts, a clock in; changes and lines out.

use crate::ambition::{new_ambition, BotNow, Stamp};
use crate::upkeep::{apply_give_back, blocked_bid_give, finish_give_back, off_trade_give, serve_query, stale_goal_give, unpaying_give, Clock, ServeFacts, UNPAID_TAG};
use crate::upkeep_ambition::{reengage_candidate, review_mark, row_verdict, RebalanceRow, RowVerdict};
use crate::upkeep_graph::{effective_state, goal_met, market_rescue, revive, spell, stuck_summary, RescueReader, RouteFacts, Spell, FINAL};
use crate::upkeep_role::{apply_retire, apply_switch, drawable_trade, operator_assigned, operator_step, retire_cancels, switch_cancels, Assigned, OperatorStep, OrderFact, Switch};
use crate::{Archetype, GoalKind, GraphState, HistoryEntry, Life, MakerRole, Node, NodeKind, NodeStatus, Profile};

const NOW: i64 = 1_800_000_000;
const CLOCK: Clock = Clock { now: NOW, tick_ms: 600, down: crate::upkeep::no_downtime };

fn life(account: &str, goal: GoalKind) -> Life {
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 11,
        profile: Profile::new(false, 0.5, 0.5, 11, Archetype::Producer),
        goal,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn with_ambition(mut life: Life, kind: &str) -> Life {
    let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    life.ambition = Some(new_ambition(kind, "random", &now, 3, &[], Stamp { tick: 0, secs: 0 }).expect("ambition"));
    life
}

/// The clock reads ticks from seconds like `brain_stuck::seconds_to_ticks`.
#[test]
fn the_clock_measures_ticks_from_seconds() {
    assert_eq!(CLOCK.ticks_since(NOW - 6), 10);
    assert_eq!(CLOCK.ticks_since(NOW + 5), 0, "a stamp in the future is zero ticks");
    assert_eq!(Clock { now: NOW, tick_ms: 0, down: crate::upkeep::no_downtime }.ticks_since(NOW - 1), 1000, "a zero tick length reads as one ms");
}

/// A give-back writes its change, names it in order, and the draw closes it.
#[test]
fn an_off_trade_answer_is_given_back_in_order() {
    let mut miner = with_ambition(life("t_priest", GoalKind::Produce { item: "mithril_ore".to_string(), count: 5 }), "priest");
    miner.goal_origin = Some("bid:mithril_ore".to_string());
    miner.history.push(entry("goal", "next:produce mithril_ore 5 (bid:mithril_ore)", NOW - 9));
    let give = off_trade_give(&miner).expect("ore is off the priest's trade");
    assert!(give.head.contains("off the priest's trade"), "{}", give.head);
    let line = apply_give_back(&mut miner, &give, NOW);
    assert!(line.ends_with(" nodes abandoned: "), "{line}");
    assert_eq!(miner.history.last().map(|last| last.outcome.as_str()), Some("bid_off_trade:bid:mithril_ore"));
    assert!(finish_give_back(&mut miner, &give, Some("g")).contains("next goal g (after giving back bid:mithril_ore)"));
    assert!(finish_give_back(&mut miner, &give, None).contains("nothing drawable after giving back"));
    let mut running = miner.clone();
    running.active = Some("n1".to_string());
    assert_eq!(off_trade_give(&running), None, "a run's report decides first");
}

/// A bid answer under one cooldown is not given back.
#[test]
fn a_fresh_bid_wait_stays() {
    let mut bid = life("t_bid", GoalKind::Produce { item: "mithril_ore".to_string(), count: 5 });
    bid.goal_origin = Some("bid:mithril_ore".to_string());
    bid.nodes.push(Node::new("n1", NodeKind::ObtainItems { item: "mithril_ore".to_string(), count: 5 }, Vec::new(), 5));
    bid.history.push(entry("goal", "next:produce mithril_ore 5 (bid:mithril_ore)", NOW - 9_000));
    bid.history.push(entry("n1", "cool_wait:gather:mining/mithril_ore", NOW - 100));
    bid.history.push(entry("cool", "cool:gather:mining/mithril_ore|6000|full", NOW - 50));
    assert_eq!(blocked_bid_give(&bid, CLOCK), None, "waited 166 ticks");
    bid.history[1] = entry("n1", "cool_wait:gather:mining/mithril_ore", NOW - 4_000);
    let give = blocked_bid_give(&bid, CLOCK).expect("past one cooldown");
    assert!(give.head.contains("waited 6666 ticks on refused sites"), "{}", give.head);
    assert!(give.history.as_deref().is_some_and(|tag| tag.starts_with("bid_released:bid:mithril_ore|n1|")));
}

/// The block's wait does not run while the daemon is down (user
/// 2026-11-23): the 6 666-tick wait above, 3 700 s of it stopped, is no
/// longer past one cooldown; the cooldown clock itself stays plain.
#[test]
fn a_bid_wait_does_not_run_while_the_daemon_is_down() {
    fn stopped(from: i64, to: i64) -> i64 {
        (to.min(NOW - 200) - from.max(NOW - 3_900)).max(0)
    }
    let mut bid = life("t_bid", GoalKind::Produce { item: "mithril_ore".to_string(), count: 5 });
    bid.goal_origin = Some("bid:mithril_ore".to_string());
    bid.nodes.push(Node::new("n1", NodeKind::ObtainItems { item: "mithril_ore".to_string(), count: 5 }, Vec::new(), 5));
    bid.history.push(entry("goal", "next:produce mithril_ore 5 (bid:mithril_ore)", NOW - 9_000));
    bid.history.push(entry("n1", "cool_wait:gather:mining/mithril_ore", NOW - 4_000));
    bid.history.push(entry("cool", "cool:gather:mining/mithril_ore|6000|full", NOW - 50));
    assert!(blocked_bid_give(&bid, CLOCK).is_some(), "no downtime: given back");
    let paused = Clock { down: stopped, ..CLOCK };
    assert_eq!(paused.awake_ticks_since(NOW - 4_000), 300 * 1000 / 600, "300 s awake of 4 000 s");
    assert_eq!(blocked_bid_give(&bid, paused), None, "300 s awake: the wait goes on");
}

/// Unpaying: no data and no rate decide nothing; below the tick's worth gives back.
#[test]
fn an_unpaying_side_goal_needs_a_read_and_a_rate() {
    let mut side = life("t_side", GoalKind::Produce { item: "diamond".to_string(), count: 50 });
    side.goal_origin = Some("side:demand:diamond".to_string());
    let query = serve_query(&side).expect("an open side demand goal");
    assert_eq!(query.item, "diamond");
    let facts = |margin, per_tick, time_value| Some(ServeFacts { margin, per_tick, time_value });
    assert_eq!(unpaying_give(&side, &query, None), None, "no margin: no decision");
    assert_eq!(unpaying_give(&side, &query, facts(50.0, 0.0, 2.0)), None, "no rate: never a verdict");
    assert_eq!(unpaying_give(&side, &query, facts(5000.0, 0.05, 0.1)), None, "pays");
    let give = unpaying_give(&side, &query, facts(1.0, 0.0005, 2.0)).expect("below a tick's worth");
    assert_eq!(give.history, Some(format!("{UNPAID_TAG}side:demand:diamond")));
    side.profile.ironman = true;
    assert_eq!(serve_query(&side), None, "an ironman has no market");
}

/// A goal drawn before the ambition changed is replaced; one linked to nothing earlier stays.
#[test]
fn a_goal_of_an_earlier_ambition_is_replaced_once() {
    let mut stale = with_ambition(life("t_stale", GoalKind::MakeMoney), "fisher");
    stale.history.push(entry("goal", "next:mining 40", NOW - 500_000));
    if let Some(held) = stale.ambition.as_mut() {
        held.revisions.push(crate::ambition::Revision { tick: 1, at_secs: NOW - 10, from: "x".to_string(), to: "fisher".to_string(), why: "t".to_string() });
    }
    let give = stale_goal_give(&stale).expect("older than the change");
    assert!(give.next.contains("{label} (fisher)"), "{}", give.next);
    assert_eq!(give.history, None);
    stale.history.push(entry("goal", "next:fishing 40", NOW - 5));
    assert_eq!(stale_goal_give(&stale), None, "the next goal is newer than the change");
}

/// The maker's switch withdraws every buy and its slice's listings only;
/// the retirement its own live quotes only.
#[test]
fn the_maker_cancels_what_its_role_asks() {
    let row = |order_id, buy, book_item, market_make, live| OrderFact { order_id, buy, book_item, market_make, live };
    let rows = vec![row(1, true, 9, false, true), row(2, false, 5, true, true), row(3, false, 7, true, true), row(4, true, 5, true, false), row(5, false, 5, false, true)];
    assert_eq!(switch_cancels(&rows, &[5]), vec![1, 2, 5]);
    assert_eq!(retire_cancels(&rows), vec![2, 3]);
    let mut maker = life("t_maker", GoalKind::MakeMoney);
    maker.profile.archetype = Archetype::MarketMaker(MakerRole::new(2, 8, 100_000));
    let role = crate::maker_role(&maker).expect("maker");
    let line = apply_switch(&mut maker, &role, 120_000, CLOCK);
    assert!(line.contains("capital 120000 >= target 100000, quoting for good"), "{line}");
    assert!(crate::quoting(&maker));
    let mut retired = life("t_maker2", GoalKind::MakeMoney);
    retired.profile.archetype = Archetype::MarketMaker(MakerRole::new(1, 8, 100_000));
    let line = apply_retire(&mut retired, 2, NOW);
    assert!(line.contains("market maker retired (2 quotes cancelled"), "{line}");
    assert!(crate::maker_role(&retired).is_none());
}

/// A filed switch: a protected account drops it, `natural` and a trade go
/// through a draft (the life changes only when the giving step succeeded).
#[test]
fn an_operator_switch_is_given_on_a_draft() {
    let zezima = life("Zezima", GoalKind::MakeMoney);
    assert!(matches!(operator_step(&zezima, "cook"), OperatorStep::Done(lines) if lines[0].contains("dropped (protected account")));
    let mut owen = with_ambition(life("t_owen", GoalKind::MakeMoney), "smith");
    assert_eq!(operator_step(&owen, "pirate"), OperatorStep::Done(Vec::new()), "an unknown kind changes nothing");
    assert!(matches!(operator_step(&owen, "alcher"), OperatorStep::Alcher));
    let before = owen.clone();
    let OperatorStep::Assign { draft, switch } = operator_step(&owen, "cook") else { panic!("a trade is given on a draft") };
    assert_eq!(switch, Switch::Trade("cook".to_string()));
    assert!(matches!(operator_assigned(&mut owen, (*draft).clone(), &switch, false, NOW), Assigned::Refused(line) if line.contains("not given")));
    assert_eq!(owen, before, "refused: untouched");
    assert!(matches!(operator_assigned(&mut owen, *draft, &switch, true, NOW), Assigned::Drawing(_)));
    assert_eq!(owen.history.last().map(|last| last.outcome.as_str()), Some("role:cook (operator)"));
    assert!(drawable_trade("cook") && !drawable_trade(crate::MARKET_MAKER_KIND) && !drawable_trade("pirate"));
    let maker = {
        let mut maker = with_ambition(life("t_m", GoalKind::MakeMoney), "smith");
        maker.profile.archetype = Archetype::MarketMaker(MakerRole::new(1, 8, 1));
        maker
    };
    assert!(matches!(operator_step(&maker, "natural"), OperatorStep::Done(lines) if lines[0].contains("natural refused")));
}

/// The D3 mark is written once and the A4 row is judged per life.
#[test]
fn the_mark_and_the_row_are_per_life_decisions() {
    let mut old = with_ambition(life("t_old", GoalKind::MakeMoney), "miner");
    if let Some(held) = old.ambition.as_mut() {
        held.draw_rule.clear();
        held.why = "need:copper_ore".to_string();
    }
    assert!(review_mark(&mut old).is_some_and(|line| line.contains("marked need (D3)")));
    assert_eq!(review_mark(&mut old), None, "once");
    let row = |from: &str, kind: &str| RebalanceRow { from: from.to_string(), kind: kind.to_string(), why: "w".to_string(), numbers: "n".to_string() };
    assert!(matches!(row_verdict(&old, &row("miner", "cook")), RowVerdict::Give));
    assert!(matches!(row_verdict(&old, &row("miner", "miner")), RowVerdict::Kept(line) if line.contains("kept miner (n)")));
    assert!(matches!(row_verdict(&old, &row("smith", "cook")), RowVerdict::Kept(line) if line.contains("row stale")));
    assert!(matches!(row_verdict(&life("t_none", GoalKind::MakeMoney), &row("a", "b")), RowVerdict::Nothing));
}

/// Seats are re-engaged only for an open bid goal.
#[test]
fn only_an_open_bid_goal_holds_seats() {
    let mut bid = life("t_seat", GoalKind::Produce { item: "mithril_ore".to_string(), count: 5 });
    assert!(!reengage_candidate(&bid));
    bid.goal_origin = Some("bid:mithril_ore".to_string());
    assert!(reengage_candidate(&bid));
    bid.history.push(entry("goal", "done:x", NOW));
    assert!(!reengage_candidate(&bid), "closed");
}

/// The stuck line stays bounded (live 2026-11-14: ~90 abandoned nodes, one line).
#[test]
fn the_stuck_line_stays_bounded() {
    let mut stuck = life("b0", GoalKind::MakeMoney);
    for number in 1..=200 {
        let mut node = Node::new(&format!("n{number}"), NodeKind::ObtainItems { item: "logs".to_string(), count: 1 }, Vec::new(), 1);
        node.status = NodeStatus::Abandoned;
        node.last_error = if number % 40 == 1 { format!("Failed: {}", "x".repeat(500)) } else { format!("dependency n{} abandoned", number - 1) };
        stuck.nodes.push(node);
    }
    let ids: Vec<String> = stuck.nodes.iter().map(|node| node.id.clone()).collect();
    let line = stuck_summary(&stuck, &ids);
    assert!(line.starts_with("200 nodes (195 by cascade); roots: n1=Failed: "), "{line}");
    assert!(line.ends_with("(+2 more roots)"), "{line}");
    assert!(line.len() < 700, "bounded: {} chars", line.len());
}

/// A spell is noted once and ends on any node event; a met skill goal is done.
#[test]
fn a_spell_is_read_back_from_the_history() {
    assert_eq!(spell(&[]), Spell::Fresh);
    let mut history = vec![entry("n24", "abandoned", 10), entry("graph", "stuck", 11), entry("goal", "stuck:woodcutting", 11)];
    assert_eq!(spell(&history), Spell::Waiting(11));
    history.push(entry("graph", "revived", 20));
    assert_eq!(spell(&history), Spell::Fresh);
    history.push(entry("graph", FINAL, 40));
    assert_eq!(spell(&history), Spell::Final);
    let level = |skill: &str| if skill == "fishing" { 16 } else { 1 };
    assert!(goal_met(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 15 }, &level));
    assert!(!goal_met(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 }, &level));
    assert!(!goal_met(&GoalKind::MakeMoney, &level));
}

/// bb4v5d9m: a reached goal over old abandons is Finished, not stuck.
#[test]
fn a_reached_goal_over_old_abandons_is_finished() {
    let mut old = Node::new("n51", NodeKind::ObtainItems { item: "leather_boots".to_string(), count: 1 }, Vec::new(), 1);
    old.status = NodeStatus::Abandoned;
    let mut equip = Node::new("n457", NodeKind::EquipBest { style: "melee".to_string() }, Vec::new(), 1);
    equip.status = NodeStatus::Done;
    let mut reached = life("b0", GoalKind::EquipBest { style: "melee".to_string() });
    reached.nodes = vec![old, equip];
    assert_eq!(effective_state(&reached, &|_| 1), GraphState::Finished);
    reached.nodes[1].status = NodeStatus::Abandoned;
    assert!(matches!(effective_state(&reached, &|_| 1), GraphState::Stuck { .. }));
}

/// The cooldown revive waits, then reopens, then stands by at the cap.
#[test]
fn the_revive_waits_for_its_cooldown() {
    let mut stuck = life("t_rev", GoalKind::MakeMoney);
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "logs".to_string(), count: 1 }, Vec::new(), 1);
    node.status = NodeStatus::Abandoned;
    stuck.nodes.push(node);
    assert_eq!(revive(&mut stuck, NOW - 100, CLOCK), None, "too early");
    let lines = revive(&mut stuck, NOW - 100_000, CLOCK).expect("past the cooldown");
    assert!(lines[0].contains("revive "), "{lines:?}");
    assert_eq!(stuck.history.last().map(|last| last.outcome.as_str()), Some("revived"));
}

struct Reader(RouteFacts);

impl RescueReader for Reader {
    fn wallet(&self) -> i64 {
        10_000
    }
    fn facts(&self, _life: &Life, _node: &Node) -> RouteFacts {
        self.0
    }
}

/// Before a ban, an item on the failed path the market can sell is bought.
#[test]
fn a_stuck_obtain_is_bought_when_the_market_has_a_route() {
    let make = || {
        let mut stuck = life("t_rescue", GoalKind::MakeMoney);
        let mut node = Node::new("n1", NodeKind::ObtainItems { item: "logs".to_string(), count: 5 }, Vec::new(), 5);
        node.status = NodeStatus::Abandoned;
        node.last_error = "blocked at resolve".to_string();
        stuck.nodes.push(node);
        stuck.history.push(entry("n1", "abandoned", NOW - 5));
        stuck
    };
    let mut lines = Vec::new();
    let mut stuck = make();
    let mut sink = |line: String| lines.push(line);
    let route = Reader(RouteFacts { best_ask: Some(10), price: 10, held: 0, committed: 0 });
    assert!(market_rescue(&mut stuck, &route, NOW, &mut sink));
    assert!(lines.last().is_some_and(|line| line.contains("bought on the market instead of banning")), "{lines:?}");
    assert_eq!(stuck.history.last().map(|last| last.outcome.as_str()), Some(crate::MARKET_RESCUE_TAG));
    let mut poor = make();
    let mut no_lines = Vec::new();
    let mut sink = |line: String| no_lines.push(line);
    let none = Reader(RouteFacts { best_ask: Some(10_000), price: 10_000, held: 0, committed: 9_000 });
    assert!(!market_rescue(&mut poor, &none, NOW, &mut sink));
    assert!(no_lines.iter().any(|line| line.contains("no market route for logs x5")), "{no_lines:?}");
}
