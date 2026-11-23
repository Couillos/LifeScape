//! Plan marché M6 in the brain: the yields read for the work slices, the
//! ceilings and the withdrawals (pure parts; the pass itself needs the
//! live market service).

use super::*;
use crate::rates_estimator::YieldBook;
use lifescape_planner::stock_target::{stock_plan, StockStep, StockView};
use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn life_of(nodes: Vec<Node>, affinity: f64) -> Life {
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

fn reach(id: &str, skill: &str, level: u8, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ReachLevel { skill: skill.to_string(), level }, Vec::new(), i64::from(level));
    node.status = status;
    node
}

fn needs_of(life: &Life, level: u8, owned: &[&str]) -> Vec<(String, f64)> {
    let level_of = |_: &str| level;
    let slices = lifescape_planner::stock_target::work_slices(life, &level_of);
    let owned: Vec<String> = owned.iter().map(|name| name.to_string()).collect();
    let consumption = slice_consumption(&YieldBook::default(), &slices, &owned, None, &level_of, 0);
    lifescape_planner::stock_target::slice_needs(&consumption)
}

fn units(needs: &[(String, f64)], item: &str) -> f64 {
    needs.iter().find(|(name, _)| name == item).map(|(_, units)| *units).unwrap_or(0.0)
}

/// M6, the user's example: a fly fisher at fishing 20 (tranche 20 → 25,
/// trout) bids its feathers, consumption per 6000 × the tranche's ticks
/// (read from the published yield: the game prior while unmeasured),
/// minus what it holds. The yield-based target matches the tranche's
/// server sizing (one feather a catch).
#[test]
fn a_fly_fisher_at_twenty_bids_the_feathers_of_its_tranche() {
    let life = life_of(vec![reach("n1", "fishing", 25, NodeStatus::Active)], 1.0);
    let needs = needs_of(&life, 20, &["fly_fishing_rod"]);
    let feathers = units(&needs, "feather");
    let sized = lifescape_planner::sizing::tranche_need("gather:fishing/raw_trout", 20, 25, "feather").unwrap_or(0) as f64;
    assert!(sized > 0.0 && (feathers - sized).abs() / sized < 0.05, "yield target {feathers} vs sizing {sized}: {needs:?}");
    let held = |item: &str| if item == "feather" { 30 } else { 0 };
    let plan = stock_plan(&needs, &StockView { held: &held, resting: &|_| 0, ceiling_of: &|_| Some(1), market_affinity: 1.0, budget: 1_000_000 });
    let line = plan.iter().find(|line| line.item == "feather").expect("a feather line");
    assert_eq!(line.step, StockStep::Bid { qty: feathers.ceil() as i64 - 30, ceiling: 1 });
}

/// Generalized (user rule: every consuming skill): a firemaker burns its
/// logs, a mage its runes, a ranger its arrows; a woodcutter burns
/// nothing and stocks nothing.
#[test]
fn every_consuming_tranche_names_its_consumables() {
    let fire = needs_of(&life_of(vec![reach("n1", "firemaking", 10, NodeStatus::Active)], 1.0), 1, &["tinderbox"]);
    assert!(units(&fire, "logs") > 0.0, "{fire:?}");
    let mage = needs_of(&life_of(vec![reach("n1", "magic", 5, NodeStatus::Active)], 1.0), 1, &[]);
    assert!(units(&mage, "mindrune") > 0.0 && units(&mage, "airrune") > 0.0, "{mage:?}");
    let woodcutter = needs_of(&life_of(vec![reach("n1", "woodcutting", 20, NodeStatus::Active)], 1.0), 15, &["bronze_axe"]);
    assert!(woodcutter.is_empty(), "{woodcutter:?}");
}

/// Ceiling: the current traded price, else the best ask, capped by the
/// cheapest keeper; a keeper alone prices it; nothing public, or under
/// the public floor, is no bid.
#[test]
fn stock_ceilings_read_public_prices() {
    assert_eq!(stock_ceiling(Some(3), Some(1), None), Some(3));
    assert_eq!(stock_ceiling(None, Some(7), Some(5)), Some(5), "the shop beyond");
    assert_eq!(stock_ceiling(None, None, Some(4)), Some(4));
    assert_eq!(stock_ceiling(None, None, None), None);
    assert_eq!(stock_ceiling(Some(2), None, None), Some(2), "no price floor");
}

fn order(order_id: i64, item: &str, motive: &str) -> lifescape_store::Order {
    lifescape_store::Order {
        order_id,
        account: "t_stock".to_string(),
        side: lifescape_store::OrderSide::Buy,
        item_id: lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64).unwrap_or(-1),
        quantity: 100,
        remaining: 100,
        price: 2,
        status: lifescape_store::OrderStatus::Open,
        dormant: false,
        created_clock: 0,
        updated_clock: 0,
        replaces: None,
        root_id: order_id,
        root_clock: 0,
        motive: motive.to_string(),
        cancel_reason: String::new(),
        ceiling: None,
        ask_limit: None,
    }
}

/// M6, plan changed: the fisher's feather stock bid goes (`need_gone`)
/// once the plan is a firemaker's; the logs bid stays; the alchemy
/// pass's nature runes, an object an open obtain wants and a need's bid
/// are never touched.
#[test]
fn a_changed_plan_withdraws_only_the_stock_bids_it_owns() {
    let firemaker = stock_plan(&[("logs".to_string(), 500.0)], &StockView { held: &|_| 0, resting: &|_| 100, ceiling_of: &|_| Some(4), market_affinity: 1.0, budget: 0 });
    let rows = vec![order(1, "feather", "stock"), order(2, "logs", "stock"), order(3, "naturerune", "stock"), order(4, "bronze_arrow", "stock"), order(5, "mindrune", "need")];
    let nature = rows[2].item_id;
    let claimed: BTreeSet<i64> = [nature].into_iter().collect();
    let wanted = vec!["bronze_arrow".to_string()];
    let stale = stale_stock(&rows, &firemaker, &claimed, &wanted, &crate::brain_gear::name_of);
    assert_eq!(stale, vec![(1, "need_gone")]);
    let autarkic = stock_plan(&[("logs".to_string(), 500.0)], &StockView { held: &|_| 0, resting: &|_| 100, ceiling_of: &|_| Some(4), market_affinity: 0.1, budget: 0 });
    let gone: Vec<i64> = stale_stock(&rows, &autarkic, &BTreeSet::new(), &[], &crate::brain_gear::name_of).into_iter().map(|(order, _)| order).collect();
    assert_eq!(gone, vec![1, 2, 3, 4], "an autarkic bot keeps no stock bid (a need's stays)");
}
