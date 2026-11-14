//! Offline sims of priced training (2026-11-14) on the real planner and
//! the brain's price wiring: a smith picks what to hammer from the book,
//! simulated mages at magic 25 and 60 (the fleet is at ≤ 10) alch their
//! stock or cheap asks instead of bolting cows, and the launch line
//! carries the batch.

use std::collections::HashMap;

use lifescape_json::{object, Value};
use lifescape_planner::alchemy::caster;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::market_fit::BookView;
use lifescape_planner::training::AlchSupply;
use lifescape_planner::{map_method, resolve_ready, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

use super::{choose_pick, TrainMarket};
use crate::brain_bids::{BotState, MarketReader};

fn live(levels: &[(&str, i64)], inv: &[(&str, i64)]) -> Value {
    let skills = levels
        .iter()
        .map(|(skill, level)| object(vec![("id", Value::Int(i64::from(lifescape_sdk::provenance::skill_id(skill).expect("skill")))), ("level", Value::Int(*level))]))
        .collect();
    let stacks = inv.iter().map(|(name, count)| object(vec![("name", lifescape_json::str_value(name)), ("count", Value::Int(*count))])).collect();
    object(vec![("skills", Value::Arr(skills)), ("inv", Value::Arr(stacks))])
}

fn life_for(skill: &str, level: u8) -> Life {
    Life {
        schema: 1,
        account: "mage01".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec![skill.to_string()], level },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn units(method: &str, _item: &str) -> f64 {
    if method.starts_with("farm:") || method.starts_with("gather:") {
        200.0
    } else {
        600.0
    }
}

/// A book of asks (other accounts) and bids per object, with current
/// prices for what traded.
fn book(asks: &[(&str, i64, i64)], bids: &[(&str, i64, i64)]) -> MarketReader {
    let mut views: HashMap<String, BookView> = HashMap::new();
    for (item, price, qty) in asks {
        views.entry(item.to_string()).or_default().asks.push((*price, *qty));
    }
    for (item, price, qty) in bids {
        views.entry(item.to_string()).or_default().bids.push((*price, *qty));
    }
    MarketReader::fixed(views)
}

const RUNE_ASKS: [(&str, i64, i64); 7] =
    [("airrune", 5, 50_000), ("waterrune", 5, 50_000), ("mindrune", 4, 50_000), ("chaosrune", 90, 20_000), ("deathrune", 160, 20_000), ("firerune", 5, 50_000), ("naturerune", 60, 20_000)];

fn traded(item: &str) -> Option<f64> {
    RUNE_ASKS.iter().find(|(name, _, _)| *name == item).map(|(_, price, _)| *price as f64)
}

/// Simulated mage at magic 25 with 300 iron daggers from its smithing:
/// low alchemy (31 XP a cast) beats water bolts (chaos runes at 90); the
/// pick casts the stock, buys the nature runes and a staff of fire (Zaff
/// sells it, D-A: nothing F2P makes one; cheaper than 3 fire runes a cast
/// over the climb) first, and the resolved launch is
/// `alchemy iron_dagger 300 --level magic 35`.
#[test]
fn magic_25_alchs_its_daggers_and_launches_the_batch() {
    let snapshot = live(&[("magic", 25), ("smithing", 30)], &[("iron_dagger", 300)]);
    let life = life_for("magic", 35);
    let level_of = |skill: &str| crate::brain_succession::live_level(&snapshot, skill);
    let held = |item: &str| crate::brain_ambition::live_count(&snapshot, item);
    let state = BotState { level_of: &level_of, held: &held, avoid: Vec::new(), quests_done: Vec::new(), units: &units, tick: 0, combat: 3, position: None, planned: Vec::new(), caster: None };
    let reader = book(&RUNE_ASKS, &[]);
    let price = |item: &str| traded(item).unwrap_or(1.0);
    let supply = vec![AlchSupply { item: "iron_dagger".to_string(), units: 300, unit_cost: 0.0, own: true }];
    let market = TrainMarket { reader: &reader, traded: &traded, mage: caster(25, false, &price), supply, coins: 40_000 };
    let pick = choose_pick("magic", 25, 35, &life, &state, &[], &market).expect("a priced pick");
    assert_eq!(pick.method, "alch:iron_dagger", "{pick:?}");
    assert_eq!(pick.batch, 300);
    assert_eq!(pick.missing, vec!["naturerune".to_string(), "staff_of_fire".to_string()]);
    // The resolver blocks on both runes at once; once held, it launches
    // the batch with the climb's level.
    let mut node = Node::new("n5", NodeKind::ReachLevel { skill: "magic".to_string(), level: 35 }, Vec::new(), 35);
    node.status = NodeStatus::Ready;
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.training = Some(pick.clone());
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:naturerune,staff_of_fire".to_string() });
    assert_eq!(lifescape_planner::replan::need_count_from(&node, "naturerune", Some(25)), 300, "one nature a cast, the batch");
    let mut ready = pick;
    ready.missing.clear();
    ctx.training = Some(ready);
    node.status = NodeStatus::Ready;
    node.method = None;
    resolve_ready(&mut node, &ctx);
    let (script, args) = map_method(node.method.as_deref().expect("locked"), &node, None).expect("maps");
    assert_eq!(script, "alchemy");
    assert_eq!(&args[..5], ["iron_dagger", "300", "--level", "magic", "35"], "{args:?}");
}

/// Simulated mage at magic 60 holding a staff of fire and nature runes,
/// nothing to alch of its own: steel platebodies resting under their high
/// alchemy value are bought to alch (fire from the staff), not fire
/// blasts at 160 a death rune.
#[test]
fn magic_60_buys_cheap_asks_to_high_alch() {
    let snapshot = live(&[("magic", 60)], &[("staff_of_fire", 1), ("naturerune", 400)]);
    let life = life_for("magic", 65);
    let level_of = |skill: &str| crate::brain_succession::live_level(&snapshot, skill);
    let held = |item: &str| crate::brain_ambition::live_count(&snapshot, item);
    let state = BotState { level_of: &level_of, held: &held, avoid: Vec::new(), quests_done: Vec::new(), units: &units, tick: 0, combat: 3, position: None, planned: Vec::new(), caster: None };
    let high = lifescape_planner::alchemy::alch_value("steel_platebody", true).expect("alchs");
    let mut asks = RUNE_ASKS.to_vec();
    asks.push(("steel_platebody", high - 60, 150));
    let reader = book(&asks, &[]);
    let price = |item: &str| traded(item).unwrap_or(1.0);
    let supply = vec![AlchSupply { item: "steel_platebody".to_string(), units: 150, unit_cost: (high - 60) as f64, own: false }];
    let market = TrainMarket { reader: &reader, traded: &traded, mage: caster(60, true, &price), supply, coins: 500_000 };
    let pick = choose_pick("magic", 60, 65, &life, &state, &[], &market).expect("a priced pick");
    assert_eq!(pick.method, "alch:steel_platebody", "{pick:?}");
    assert_eq!(pick.batch, 150, "the whole resting supply");
    assert_eq!(pick.missing, vec!["steel_platebody".to_string()], "runes held, fire from the staff");
    // No supply at all: fire blast (the staff pays its fire runes).
    let bare = TrainMarket { reader: &reader, traded: &traded, mage: caster(60, true, &price), supply: Vec::new(), coins: 500_000 };
    let fight = choose_pick("magic", 60, 65, &life, &state, &[], &bare).expect("combat pick");
    assert_eq!(fight.method, "fight:magic/fire_blast/cows");
}

/// A smith at 18 (mining 1) with the live scimitar bids (2961 down to 700)
/// and bronze bars resting at 5: scimitars pay, not the platebody; without
/// bids the platebody's 5 bars a press win; the hammer and bars are
/// obtained first. A miner (mining 20) with a cheap hour mines and smelts
/// its own iron instead (25 XP a bar plus 12.5 at the furnace).
#[test]
fn smith_18_hammers_what_the_book_pays() {
    let miner = live(&[("smithing", 18), ("mining", 20)], &[("bronze_pickaxe", 1)]);
    let life = life_for("smithing", 20);
    {
        let level_of = |skill: &str| crate::brain_succession::live_level(&miner, skill);
        let held = |item: &str| crate::brain_ambition::live_count(&miner, item);
        let state = BotState { level_of: &level_of, held: &held, avoid: Vec::new(), quests_done: Vec::new(), units: &units, tick: 0, combat: 3, position: None, planned: Vec::new(), caster: None };
        let none = |_: &str| None;
        let quiet = book(&[("bronze_bar", 5, 400)], &[]);
        let plain = TrainMarket { reader: &quiet, traded: &none, mage: None, supply: Vec::new(), coins: 5_000 };
        let pick = choose_pick("smithing", 18, 20, &life, &state, &[], &plain).expect("a priced pick");
        assert!(pick.method.starts_with("smith:iron_"), "{pick:?}");
        assert_eq!(pick.missing, vec!["iron_bar".to_string()]);
    }
    let snapshot = live(&[("smithing", 18), ("mining", 1)], &[]);
    let level_of = |skill: &str| crate::brain_succession::live_level(&snapshot, skill);
    let held = |item: &str| crate::brain_ambition::live_count(&snapshot, item);
    let state = BotState { level_of: &level_of, held: &held, avoid: Vec::new(), quests_done: Vec::new(), units: &units, tick: 0, combat: 3, position: None, planned: Vec::new(), caster: None };
    let none = |_: &str| None;
    let scimitars: Vec<(&str, i64, i64)> = [2961, 2896, 2876, 2484, 2434, 2133, 1967, 1930, 1774, 1500].iter().map(|price| ("bronze_scimitar", *price, 1)).collect();
    let reader = book(&[("bronze_bar", 5, 400)], &scimitars);
    let market = TrainMarket { reader: &reader, traded: &none, mage: None, supply: Vec::new(), coins: 5_000 };
    let pick = choose_pick("smithing", 18, 20, &life, &state, &[], &market).expect("a priced pick");
    assert_eq!(pick.method, "smith:bronze_scimitar", "{pick:?}");
    assert_eq!(pick.missing, vec!["bronze_bar".to_string()]);
    let quiet = book(&[("bronze_bar", 5, 400)], &[]);
    let plain = TrainMarket { reader: &quiet, traded: &none, mage: None, supply: Vec::new(), coins: 5_000 };
    let pick = choose_pick("smithing", 18, 20, &life, &state, &[], &plain).expect("a priced pick");
    assert_eq!(pick.method, "smith:bronze_platebody", "{pick:?}");
    let mut node = Node::new("n2", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 20 }, Vec::new(), 20);
    node.status = NodeStatus::Ready;
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.training = Some(pick);
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:hammer,bronze_bar".to_string() });
}
