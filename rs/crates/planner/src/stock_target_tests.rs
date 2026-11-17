//! Plan marché M6: future stock (pure). The brain-side glue (rates read,
//! book, bids) lives in `orchestrator::brain_stock`.

use super::*;
use crate::model::{Archetype, Engagement, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};
use crate::sizing::xp_between;

fn life_with(nodes: Vec<Node>, affinity: f64) -> Life {
    Life {
        schema: 1,
        account: "t_stock".to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, affinity, 0.5, 1, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn reach(id: &str, skill: &str, level: u8, status: NodeStatus, deps: &[&str]) -> Node {
    let mut node = Node::new(id, NodeKind::ReachLevel { skill: skill.to_string(), level }, deps.iter().map(|dep| dep.to_string()).collect(), i64::from(level));
    node.status = status;
    node
}

/// A fly-fishing tranche at 20 → 25 and the next one: the active node
/// first, the next tranche starts where the current one ends.
#[test]
fn the_current_tranche_and_the_next_are_the_work_slices() {
    let nodes = vec![
        reach("n1", "fishing", 25, NodeStatus::Active, &[]),
        reach("n2", "fishing", 30, NodeStatus::Pending, &["n1"]),
        reach("n3", "fishing", 35, NodeStatus::Pending, &["n2"]),
    ];
    let life = life_with(nodes, 0.9);
    let slices = work_slices(&life, &|skill| if skill == "fishing" { 21 } else { 1 });
    assert_eq!(slices.len(), STOCK_SLICES, "{slices:?}");
    assert_eq!((slices[0].node.as_str(), slices[0].from, slices[0].to), ("n1", 21, 25));
    assert_eq!(slices[0].method, "gather:fishing/raw_trout");
    assert_eq!((slices[1].node.as_str(), slices[1].from, slices[1].to), ("n2", 25, 30));
    assert_eq!(slices[1].method, "gather:fishing/raw_pike");
}

/// Done and abandoned nodes are no work; a tranche the live level already
/// reached is none either; a money node works its drawn engagement.
#[test]
fn finished_work_is_no_slice_and_money_works_its_engagement() {
    let mut money = Node::new("n9", NodeKind::MakeMoney, Vec::new(), 0);
    money.status = NodeStatus::Ready;
    money.method = Some("fire:logs".to_string());
    let nodes = vec![
        reach("n1", "fishing", 25, NodeStatus::Done, &[]),
        reach("n2", "fishing", 30, NodeStatus::Abandoned, &[]),
        reach("n3", "cooking", 10, NodeStatus::Ready, &[]),
        money,
    ];
    let mut life = life_with(nodes, 0.9);
    let slices = work_slices(&life, &|skill| if skill == "cooking" { 12 } else { 1 });
    assert_eq!(slices.len(), 1, "{slices:?}");
    assert_eq!(slices[0].window_ticks, Some(f64::from((crate::replan::ENGAGE_MIN + crate::replan::ENGAGE_MAX) as u32) / 2.0), "undrawn: the mean of the draw");
    life.engagement = Some(Engagement { node: "n9".to_string(), min_ticks: 6000, max_ticks: 15_000, chosen: 15_000 });
    assert_eq!(work_slices(&life, &|_| 12)[0].window_ticks, Some(15_000.0), "drawn: its engagement");
}

/// A slice lasts the XP it must earn at the method's XP per 6000 ticks
/// (a window when it has one); no XP rate, no duration.
#[test]
fn a_slice_lasts_its_xp_at_its_rate() {
    let slice = WorkSlice { node: "n1".to_string(), method: "gather:fishing/raw_trout".to_string(), skill: Some("fishing".to_string()), from: 20, to: 25, window_ticks: None };
    let xp = xp_between(20, 25);
    assert_eq!(slice_ticks(&slice, Some(xp)), Some(6000.0), "one window of XP");
    assert_eq!(slice_ticks(&slice, Some(xp / 2.0)), Some(12_000.0));
    assert_eq!(slice_ticks(&slice, Some(0.0)), None);
    assert_eq!(slice_ticks(&slice, None), None);
    let money = WorkSlice { window_ticks: Some(9000.0), ..slice };
    assert_eq!(slice_ticks(&money, None), Some(9000.0));
}

/// Consumption = the signed outflows of the consumable causes (input,
/// ammo, rune, bait, supply, food); products, loot and fees are not
/// stock. Food eaten counts as hitpoints in the chosen food.
#[test]
fn consumption_reads_the_consumable_outflows() {
    let flows = vec![
        ("raw_trout".to_string(), "product".to_string(), 300.0),
        ("feather".to_string(), "bait".to_string(), -300.0),
        ("coins".to_string(), "fee".to_string(), -20.0),
        ("mindrune".to_string(), "rune".to_string(), -50.0),
        ("shrimp".to_string(), "food".to_string(), -30.0),
        ("bones".to_string(), "loot".to_string(), 40.0),
    ];
    let plain = consumed_per_6000(&flows, None);
    assert_eq!(plain, vec![("feather".to_string(), 300.0), ("mindrune".to_string(), 50.0), ("shrimp".to_string(), 30.0)]);
    // 30 shrimps eaten = their hitpoints, in trout.
    let as_trout = consumed_per_6000(&flows, Some("trout"));
    let trout = as_trout.iter().find(|(item, _)| item == "trout").map(|(_, units)| *units).unwrap_or(0.0);
    let heal = |food: &str| lifescape_gamedata::food::heal(food) as f64;
    assert!((trout - 30.0 * heal("shrimp") / heal("trout")).abs() < 1e-9, "{as_trout:?}");
    assert!(!as_trout.iter().any(|(item, _)| item == "shrimp"));
}

fn view<'a>(held: &'a dyn Fn(&str) -> i64, resting: &'a dyn Fn(&str) -> i64, ceiling_of: &'a dyn Fn(&str) -> Option<i64>, affinity: f64, budget: i64) -> StockView<'a> {
    StockView { held, resting, ceiling_of, market_affinity: affinity, budget }
}

/// M6 test 1: a fly fisher 20 → 25 with a market affinity of 1 bids its
/// feathers: consumption per 6000 × the slice's ticks, minus what it
/// holds free.
#[test]
fn a_fly_fisher_bids_its_feathers_minus_what_it_holds() {
    let consumed = vec![("feather".to_string(), 400.0)];
    let needs = slice_needs(&[(consumed, 9000.0)]);
    assert_eq!(needs, vec![("feather".to_string(), 600.0)]);
    let held = |item: &str| if item == "feather" { 150 } else { 0 };
    let plan = stock_plan(&needs, &view(&held, &|_| 0, &|_| Some(2), 1.0, 100_000));
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].target, 600);
    assert_eq!(plan[0].held, 150);
    assert_eq!(plan[0].step, StockStep::Bid { qty: 450, ceiling: 2 });
}

/// M6 test 2: an autarkic bot (affinity under `AUTARKY_BELOW`) farms: no
/// stock line, so no bid (and any resting stock bid goes).
#[test]
fn an_autarkic_bot_bids_nothing() {
    let needs = vec![("feather".to_string(), 600.0)];
    assert!(stock_plan(&needs, &view(&|_| 0, &|_| 0, &|_| Some(2), 0.1, 100_000)).is_empty());
    assert_eq!(bought_share(AUTARKY_BELOW), 0.0);
    assert_eq!(bought_share(1.0), 1.0);
    let half = stock_plan(&needs, &view(&|_| 0, &|_| 0, &|_| Some(2), (1.0 + AUTARKY_BELOW) / 2.0, 100_000));
    assert_eq!(half[0].target, 300, "half way to market-loving: half the target");
}

/// M6 test 3: the budget caps the bids, the current slice's items first.
#[test]
fn the_budget_caps_the_bids() {
    let needs = slice_needs(&[(vec![("mindrune".to_string(), 600.0)], 6000.0), (vec![("airrune".to_string(), 600.0)], 6000.0)]);
    let plan = stock_plan(&needs, &view(&|_| 0, &|_| 0, &|_| Some(4), 1.0, 1000));
    assert_eq!(plan[0].item, "mindrune");
    assert_eq!(plan[0].step, StockStep::Bid { qty: 250, ceiling: 4 }, "1000 coins at 4");
    assert_eq!(plan[1].step, StockStep::Short, "nothing left for the next slice");
    let broke = stock_plan(&needs, &view(&|_| 0, &|_| 0, &|_| Some(4), 1.0, 3));
    assert!(broke.iter().all(|line| line.step == StockStep::Short), "{broke:?}");
}

/// Refill threshold: above half its target the bot waits; at its target
/// the need is met; a resting stock bid is kept (its walk goes on); an
/// unpriced object is never bid.
#[test]
fn refill_threshold_met_kept_and_unpriced() {
    let needs = vec![("feather".to_string(), 600.0)];
    let at = |count: i64| move |_: &str| count;
    let wait = stock_plan(&needs, &view(&at(400), &|_| 0, &|_| Some(2), 1.0, 100_000));
    assert_eq!(wait[0].step, StockStep::Wait, "400 of 600: over the refill point");
    let refill = stock_plan(&needs, &view(&at(250), &|_| 0, &|_| Some(2), 1.0, 100_000));
    assert_eq!(refill[0].step, StockStep::Bid { qty: 350, ceiling: 2 });
    let met = stock_plan(&needs, &view(&at(600), &|_| 120, &|_| Some(2), 1.0, 100_000));
    assert_eq!(met[0].step, StockStep::Met);
    let kept = stock_plan(&needs, &view(&at(100), &|_| 200, &|_| Some(3), 1.0, 0));
    assert_eq!(kept[0].step, StockStep::Keep { ceiling: Some(3) }, "a resting bid needs no new budget");
    let unpriced = stock_plan(&needs, &view(&at(0), &|_| 0, &|_| None, 1.0, 100_000));
    assert_eq!(unpriced[0].step, StockStep::Unpriced);
}

/// M6 test 4: the plan changed (the fisher became a firemaker): the
/// feather line is gone, its resting stock bid is withdrawn; a met line
/// withdraws its bid as served; kept lines stay.
#[test]
fn a_changed_plan_withdraws_the_stock_bid() {
    let fisher = stock_plan(&[("feather".to_string(), 600.0)], &view(&|_| 0, &|_| 300, &|_| Some(2), 1.0, 0));
    assert_eq!(withdrawal("feather", &fisher), None, "still planned: kept");
    let firemaker = stock_plan(&[("logs".to_string(), 600.0)], &view(&|_| 0, &|_| 0, &|_| Some(5), 1.0, 100_000));
    assert_eq!(withdrawal("feather", &firemaker), Some(NEED_GONE));
    assert_eq!(withdrawal("logs", &firemaker), None);
    let served = stock_plan(&[("logs".to_string(), 600.0)], &view(&|_| 700, &|_| 50, &|_| Some(5), 1.0, 0));
    assert_eq!(withdrawal("logs", &served), Some(NEED_MET));
}
