//! Offline sims of the 2026-11-14 market rules (point 7 and "bots play
//! like real players"), on the real planner and market code:
//! - a smith with the live bronze scimitar bids draws a production quota
//!   (`bid:`), resolves it to the smithing leg with bars bought at the
//!   public ask, and its listing fills the bids it counted;
//! - a firemaker without any willow demand keeps its logs for its own
//!   fire (never surplus), a plain woodcutter lists only what buyers take,
//!   (a money draw reads the guide's profits, which price the glut, D12:
//!   `brain_d17_tests`).

use std::collections::{HashMap, HashSet};

use lifescape_json::{object, Value};
use lifescape_planner::bid_response::{answers_now, bid_goal, BID_PREFIX};
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::market_fit::{sale_lot, BookView};
use lifescape_planner::{
    append_goal, mark_drawn, new_ambition, normalize, record_block_from, resolve_ready, Archetype, BotNow, GoalKind, Life, NodeKind, NodeStatus, Profile,
    Stamp,
};

use crate::brain_bids::{choose, BotState, MarketReader};

/// Live 2026-11-14: 18 one-unit bronze scimitar bids, 2961 down to 700.
const SCIMITAR_BIDS: [i64; 18] = [2961, 2896, 2876, 2484, 2434, 2133, 1967, 1930, 1774, 1500, 1400, 1300, 1200, 1100, 1000, 900, 800, 700];

fn live(levels: &[(&str, i64)], inv: &[(&str, i64)]) -> Value {
    let skills = levels
        .iter()
        .map(|(skill, level)| object(vec![("id", Value::Int(i64::from(lifescape_sdk::provenance::skill_id(skill).expect("skill")))), ("level", Value::Int(*level))]))
        .collect();
    let stacks = inv.iter().map(|(name, count)| object(vec![("name", lifescape_json::str_value(name)), ("count", Value::Int(*count))])).collect();
    object(vec![("skills", Value::Arr(skills)), ("inv", Value::Arr(stacks))])
}

fn life_of(kind: &str, seed: u64, level_of: &dyn Fn(&str) -> u8) -> Life {
    let profile = Profile::new(false, 0.5, 0.5, seed, Archetype::Producer);
    let now = BotNow { level_of, coins: 500, held: &|_| 0 };
    Life {
        schema: 1,
        account: "smith01".to_string(),
        created_at: 0,
        seed,
        profile,
        goal: GoalKind::MaxSkills { skills: vec!["attack".to_string()], level: 20 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: new_ambition(kind, "random", &now, seed, &[], Stamp { tick: 0, secs: 0 }).ok(),
        goal_origin: None,
        memory: Default::default(),
    }
}

fn scimitar_market() -> MarketReader {
    let mut views = HashMap::new();
    views.insert("bronze_scimitar".to_string(), BookView { bids: SCIMITAR_BIDS.iter().map(|price| (*price, 1)).collect(), asks: Vec::new(), volume_24h: 0, ..BookView::default() });
    views.insert("bronze_bar".to_string(), BookView { bids: Vec::new(), asks: vec![(5, 146)], volume_24h: 40, ..BookView::default() });
    MarketReader::fixed(views)
}

fn sim_units(method: &str, _item: &str) -> f64 {
    if method.starts_with("farm:") { 200.0 } else { 600.0 }
}

/// Point 7, end to end: the live bids pay ~300× the bars; a warrior with
/// smithing 20 and mining 20 answers them on its draw (ambition kept),
/// the quota resolves to smithing with a standing bid on the bars at 5, and the default
/// listing (opening at the best bid, 5 % steps per window) fills exactly
/// the 9 bids it counted, at their prices.
#[test]
fn a_smith_answers_the_scimitar_bids_and_fills_them() {
    let levels = [("smithing", 20), ("mining", 20), ("attack", 20), ("strength", 20), ("defence", 20)];
    let snapshot = live(&levels, &[("hammer", 1)]);
    let level_of = |skill: &str| crate::brain_succession::live_level(&snapshot, skill);
    let seed = (1..200).find(|seed| answers_now(*seed, "bronze_scimitar", 0)).expect("a seed answers window 0");
    let mut life = life_of("warrior", seed, &level_of);
    let held = |item: &str| crate::brain_ambition::live_count(&snapshot, item);
    let state = BotState { level_of: &level_of, held: &held, avoid: Vec::new(), quests_done: Vec::new(), units: &sim_units, tick: 0, combat: 3, position: None, planned: Vec::new(), caster: None, rate: &lifescape_planner::work_rate::prior_rate, seats: Vec::new(), cooled: Vec::new(), shunned: Vec::new() };
    let reader = scimitar_market();
    let (found, rate) = choose(&life, &reader, crate::brain_bids::bot_rate(&life), &state).expect("the bids pay");
    assert!((rate - lifescape_planner::market_fit::MIN_COIN_RATE).abs() < 1e-9, "no money guide published: time at the floor");
    assert_eq!(found.count, 9, "half the 18 profitable bids: {found:?}");
    let (goal, origin) = bid_goal(&found);
    assert_eq!(origin, format!("{BID_PREFIX}bronze_scimitar"));
    let stones = |life: &Life| life.ambition.as_ref().map(|ambition| ambition.milestones.iter().map(|stone| (stone.id.clone(), stone.target.clone())).collect::<Vec<_>>());
    let before = stones(&life);
    let profile = life.profile.clone();
    append_goal(&mut life, goal.clone(), &profile, &level_of, &held);
    mark_drawn(&mut life, Some(&origin));
    assert_eq!(life.goal, GoalKind::Produce { item: "bronze_scimitar".to_string(), count: 9 });
    assert_eq!(stones(&life), before, "the ambition is untouched");

    // Resolve the quota like the launch pass: production legs only.
    normalize(&mut life);
    let quota = life.nodes.iter().position(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "bronze_scimitar")).expect("quota node");
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.levels = levels.iter().map(|(skill, level)| (skill.to_string(), *level as u8)).collect();
    ctx.tools = vec!["hammer".to_string()];
    ctx.coins = 500;
    resolve_ready(&mut life.nodes[quota], &ctx);
    assert_eq!(life.nodes[quota].method.as_deref(), Some("smith:bronze_scimitar"), "{:?}", life.nodes[quota]);
    // The smith script blocks on the bars: the sub-task buys them at 5.
    record_block_from(&mut life, quota, &["bronze_bar".to_string()], &level_of);
    normalize(&mut life);
    let bars = life.nodes.iter().position(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "bronze_bar")).expect("bars sub-task");
    let mut bar_ctx = ctx.clone();
    bar_ctx.best_ask = Some(5);
    bar_ctx.price = 5;
    resolve_ready(&mut life.nodes[bars], &bar_ctx);
    // D-B: the standing bid takes the 5-coin ask at once while the smith
    // smelts meanwhile; whichever lands first closes the sub-task.
    assert!(life.nodes[bars].alternatives.iter().any(|alt| alt.method == "market:bid"), "bars at the public ask: {:?}", life.nodes[bars]);
    for node in &mut life.nodes {
        node.status = NodeStatus::Done;
    }

    // Sell: the default listing opens at the best bid and walks.
    let mut book = lifescape_market::Book::new();
    for (index, price) in SCIMITAR_BIDS.iter().enumerate() {
        book.place(index as u64 + 1, &format!("fighter{index}"), 1321, lifescape_market::Side::Buy, 1, *price).expect("bid");
    }
    let mut left = found.count;
    let mut price = lifescape_market::opening_ask(1, book.best().0);
    let mut fills: Vec<i64> = Vec::new();
    let mut next_id = 100;
    for _window in 0..400 {
        if left == 0 {
            break;
        }
        next_id += 1;
        let (got, rests) = book.place(next_id, "smith01", 1321, lifescape_market::Side::Sell, left, price).expect("ask");
        let sold: i64 = got.iter().map(|fill| fill.quantity).sum();
        fills.extend(got.iter().map(|fill| fill.price));
        left -= sold;
        if rests {
            book.cancel(next_id).expect("cancel the rest before repricing");
        }
        price = lifescape_market::demand_step(price, true, sold > 0, 1, i64::MAX / 4);
    }
    assert_eq!(left, 0, "the lot sells into the bids: {fills:?}");
    let revenue: i64 = fills.iter().sum();
    assert_eq!(revenue as f64, found.revenue, "fills at the bids the quota counted: {fills:?}");
    assert!(fills.iter().all(|price| *price as f64 > found.unit.total(rate)), "every unit above its cost");
}

/// "Bots play like real players" (2026-11-14): no willow demand, 33 364
/// willow logs offered at 1 coin. A firemaker keeps its logs for its own
/// climb; a plain woodcutter lists only what buyers take (the money
/// draw reads the guide's profits: `brain_d17_tests`).
#[test]
fn without_willow_demand_logs_are_burned_not_dumped() {
    let firemaking = |skill: &str| match skill {
        "woodcutting" => 35,
        "firemaking" => 30,
        _ => 1,
    };
    let firemaker = life_of("firemaker", 5, &firemaking);
    let stacks = vec![("willow_logs".to_string(), 2000)];
    let keep = crate::brain_sales::working_keep(&firemaker, &[]);
    let offered = crate::brain_sales::surplus(&stacks, &keep, &[], &HashSet::new());
    assert!(offered.is_empty(), "its fire burns them: {keep:?} / {offered:?}");
    let woodcutter = life_of("woodcutter", 5, &firemaking);
    let keep = crate::brain_sales::working_keep(&woodcutter, &[]);
    let offered = crate::brain_sales::surplus(&stacks, &keep, &[], &HashSet::new());
    assert_eq!(offered.iter().map(|(name, _, qty)| (name.as_str(), *qty)).collect::<Vec<_>>(), vec![("willow_logs", 2000)], "a woodcutter sells its logs");
    let glut = BookView { bids: Vec::new(), asks: vec![(1, 33_364)], volume_24h: 4_319, ..BookView::default() };
    assert_eq!(sale_lot(2000, Some(&glut)), 2000.min(4_319), "a day of volume");
    let thin = BookView { bids: Vec::new(), asks: vec![(1, 33_364)], volume_24h: 0, ..BookView::default() };
    assert_eq!(sale_lot(2000, Some(&thin)), lifescape_planner::market_fit::SALE_LOT_MIN, "one inventory into a dead market");
}

/// The market-wide choice (2026-11-14): bst29bh9 (smithing 22, mining 55,
/// iron ore in the bank) reads the whole book: 3 iron full helm bids at
/// 123, iron daggers trading at 60. It takes a lot; a second smith reads
/// the first one's claim on the board and its lot shrinks. The bars its
/// graph would buy carry a reservation that rises with the helm.
#[test]
fn smiths_weigh_the_whole_market_and_claims_share_it() {
    let levels = [("smithing", 22), ("mining", 55), ("attack", 20), ("strength", 20), ("defence", 20)];
    let snapshot = live(&levels, &[("hammer", 1), ("iron_ore", 60)]);
    let level_of = |skill: &str| crate::brain_succession::live_level(&snapshot, skill);
    let held = |item: &str| crate::brain_ambition::live_count(&snapshot, item);
    let book = |claimed: i64| {
        let mut views = HashMap::new();
        views.insert("iron_full_helm".to_string(), BookView { bids: vec![(123, 3)], claimed, ..BookView::default() });
        views.insert("iron_dagger".to_string(), BookView { volume_24h: 400, current: Some(60), ..BookView::default() });
        MarketReader::fixed(views)
    };
    let picks = |claimed: i64| {
        let reader = book(claimed);
        (0..400u64).find_map(|seed| {
            let life = life_of("smith", seed, &level_of);
            let state = BotState { level_of: &level_of, held: &held, avoid: Vec::new(), quests_done: Vec::new(), units: &sim_units, tick: 0, combat: 30, position: Some((3185, 3436, 0)), planned: Vec::new(), caster: None, rate: &lifescape_planner::work_rate::prior_rate, seats: Vec::new(), cooled: Vec::new(), shunned: Vec::new() };
            choose(&life, &reader, lifescape_planner::market_fit::MIN_COIN_RATE, &state).filter(|(found, _)| found.item == "iron_full_helm").map(|(found, _)| found)
        })
    };
    let first = picks(0).expect("a smith answers the helm bids");
    assert_eq!(first.count, 2, "{first:?}");
    assert!(first.unit.plan.iter().any(|step| step == "held iron_ore"), "its own ore: {first:?}");
    let second = picks(first.count).expect("one bid left");
    assert_eq!(second.count, 1, "the claim is supply on its way: {second:?}");
    assert_eq!(picks(3), None, "every bid claimed");

    // Derived demand: the bars of that lot, priced from the helm.
    let reserve_at = |helm: f64| {
        let mut top = lifescape_planner::Node::new("n1", NodeKind::ObtainItems { item: "iron_full_helm".to_string(), count: 2 }, vec!["n2".to_string()], 2);
        top.method = Some("smith:iron_full_helm".to_string());
        let bars = lifescape_planner::Node::new("n2", NodeKind::ObtainItems { item: "iron_bar".to_string(), count: 4 }, Vec::new(), 4);
        let value = move |item: &str| (item == "iron_full_helm").then_some(helm);
        let input = |_: &str| 1.0;
        let prices = lifescape_planner::derived_demand::DerivedPrices { value_of: &value, input_price: &input, units_per_6000: &sim_units, coin_rate: 0.1 };
        lifescape_planner::derived_demand::input_reserve(&[top, bars], 1, &prices).expect("derived")
    };
    let (open_low, ceiling_low) = lifescape_planner::derived_demand::derived_bounds(reserve_at(123.0), 500);
    let (open_high, ceiling_high) = lifescape_planner::derived_demand::derived_bounds(reserve_at(1500.0), 5000);
    assert!(open_low >= 20 && ceiling_low >= 40, "a 2-coin bar bid becomes {open_low}..{ceiling_low}");
    assert!(open_high > 10 * open_low / 2 && ceiling_high > ceiling_low * 5, "{open_low}..{ceiling_low} → {open_high}..{ceiling_high}");
}
