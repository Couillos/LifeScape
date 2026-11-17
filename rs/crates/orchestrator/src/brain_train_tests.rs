//! The brain's training pick read from a precomputed guide set (R6): a
//! set built in memory (no store, no engine, no rate read: a pick is a
//! READ of the published views), fabricated lines, profiles of every
//! trait; the resolver then locks the pick and blocks on what it lacks.

use std::collections::HashMap;
use std::time::Instant;

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_model::{Confidence, Family, MoneyRow, Prereqs, SourceGap, Supply, TrainingRow};
use lifescape_planner::guide_read::{MoneyView, TrainingView};
use lifescape_planner::{resolve_ready, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

use super::pick_from_guide;
use crate::guides_store::GuideSet;

fn life_with(market_affinity: f64, thrift: f64) -> Life {
    Life {
        schema: 1,
        account: "maker01".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, market_affinity, 0.5, 7, Archetype::Producer).with_thrift(thrift),
        goal: GoalKind::MaxSkills { skills: vec!["firemaking".to_string()], level: 65 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn line(skill: &str, method: &str, supply: Supply, xp_6000: f64, coins_6000: f64) -> TrainingRow {
    TrainingRow {
        skill: skill.to_string(),
        band: 56,
        method: method.to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        supply,
        prereqs: Prereqs::default(),
        xp_6000,
        coins_6000,
        coins_per_xp: -coins_6000 / xp_6000,
        converted_ticks_6000: 0.0,
        effective_xp_6000: xp_6000,
        ticks_to_band_end: None,
        other_xp_6000: Vec::new(),
        quest_xp: 0.0,
        quest_ticks: None,
        flows: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        priced: true,
    }
}

/// The bot's best money line (D17): 30 000 coins per 6000 ticks.
fn money_line() -> MoneyRow {
    MoneyRow {
        family: Family::Farm,
        method: "gather:woodcut/logs".to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 56,
        prereqs: Prereqs::default(),
        flows: Vec::new(),
        other_drops: Default::default(),
        revenue_6000: 30_000.0,
        cost_6000: 0.0,
        profit_6000: 30_000.0,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        priced: true,
    }
}

/// A published set whose band 56-60 holds `rows` (built at 60).
fn set_with(rows: Vec<TrainingRow>) -> GuideSet {
    let money = MoneyView { level: Some(60), rows: vec![money_line()], ..MoneyView::default() };
    GuideSet {
        built_unix_ms: 0,
        built: Instant::now(),
        cycle_ms: 0,
        cycle_cpu_ms: None,
        market_wait_ticks: None,
        money_catalog: MoneyView::default(),
        training_catalog: TrainingView::default(),
        money_bands: vec![(60, money)],
        training_bands: vec![(60, TrainingView { level: Some(60), rows, gaps: Vec::new() })],
        quotes: HashMap::new(),
        stats: lifescape_json::Value::Null,
    }
}

/// Firemaking yew from the guide (R6 example): the market bot buys its
/// logs (the faster line once its coins are converted), the autarkic
/// one cuts them; a quest shortcut and another skill's line never
/// launch a firemaking climb.
#[test]
fn the_brain_picks_its_line_from_the_published_guide() {
    let mut quest = line("firemaking", "quest:druidic_ritual", Supply::Quest, 1_000_000.0, 0.0);
    quest.quest_xp = 250.0;
    let set = set_with(vec![
        quest,
        line("woodcutting", "gather:woodcut/yew_logs", Supply::Autonomous, 90_000.0, 30_000.0),
        line("firemaking", "fire:yew_logs", Supply::Autonomous, 30_000.0, 0.0),
        line("firemaking", "fire:yew_logs", Supply::Market, 60_000.0, -30_000.0),
    ]);
    let none = |_: &str| 0;
    let market = pick_from_guide(&set, "firemaking", 60, 65, &life_with(1.0, 0.5), &[], &none).expect("a line");
    assert_eq!((market.pick.method.as_str(), market.supply), ("fire:yew_logs", Supply::Market));
    assert_eq!(market.best_profit, Some(30_000.0), "D17 read in the money view");
    assert_eq!(market.scored, 2, "the quest and the woodcutting line are out");
    assert_eq!(market.pick.missing, vec!["yew_logs".to_string()], "the logs to obtain first");
    let autarkic = pick_from_guide(&set, "firemaking", 60, 65, &life_with(0.0, 0.5), &[], &none).expect("a line");
    assert_eq!((autarkic.pick.method.as_str(), autarkic.supply), ("fire:yew_logs", Supply::Autonomous));
    assert!((autarkic.pick.xp_per_tick - 5.0).abs() < 1e-9, "the line's effective rate");
}

/// Rapide ↔ économe on two magic lines: the fast bot teleports (dear,
/// quick), the thrifty one curses (cheap, slow). Inaccessible or
/// blocked lines never win, and with none left the pick is a typed
/// error (the static tranche then runs).
#[test]
fn the_thrift_trait_splits_a_fast_line_from_a_cheap_one() {
    let mut blocked = line("magic", "fight:magic/fire_strike/cows", Supply::Autonomous, 900_000.0, 0.0);
    blocked.prereqs.sources = vec![SourceGap { item: "firerune".to_string(), reason: "no autonomous source".to_string() }];
    let set = set_with(vec![
        blocked,
        line("magic", "teleport:varrock_teleport", Supply::Market, 120_000.0, -120_000.0),
        line("magic", "curse:confuse/chickens", Supply::Market, 30_000.0, -3000.0),
    ]);
    let none = |_: &str| 0;
    let fast = pick_from_guide(&set, "magic", 58, 60, &life_with(0.5, 0.0), &[], &none).expect("a line");
    assert_eq!(fast.pick.method, "teleport:varrock_teleport");
    let thrifty = pick_from_guide(&set, "magic", 58, 60, &life_with(0.5, 1.0), &[], &none).expect("a line");
    assert_eq!(thrifty.pick.method, "curse:confuse/chickens");
    let empty = set_with(Vec::new());
    assert!(pick_from_guide(&empty, "magic", 58, 60, &life_with(0.5, 0.5), &[], &none).is_err());
}

/// The pick locks like any priced tranche: the resolver blocks on the
/// logs it lacks, then locks the method once they are held.
#[test]
fn the_resolver_locks_the_guide_pick() {
    let set = set_with(vec![line("firemaking", "fire:yew_logs", Supply::Market, 60_000.0, -30_000.0)]);
    let none = |_: &str| 0;
    let found = pick_from_guide(&set, "firemaking", 60, 65, &life_with(1.0, 0.5), &[], &none).expect("a line");
    let mut node = Node::new("n4", NodeKind::ReachLevel { skill: "firemaking".to_string(), level: 65 }, Vec::new(), 65);
    node.status = NodeStatus::Ready;
    let mut ctx = ReadyCtx::blind(1.0);
    ctx.bank_known = true;
    ctx.tools = vec!["tinderbox".to_string()];
    ctx.training = Some(found.pick.clone());
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some("fire:yew_logs"));
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:yew_logs".to_string() });
    let logs = |item: &str| if item == "yew_logs" { 27 } else { 0 };
    let held = pick_from_guide(&set, "firemaking", 60, 65, &life_with(1.0, 0.5), &[], &logs).expect("a line");
    assert!(held.pick.missing.is_empty(), "a load in hand");
}
