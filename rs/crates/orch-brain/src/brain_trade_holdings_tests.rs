#![forbid(unsafe_code)]
//! Lot 1.1 proof: `brain_trade::publish_on` reads the holdings once
//! (`Holdings::of`); the figures it publishes (`ctx.trades`, the wallet)
//! are those of the former body, which asked `holdings_count` for every
//! object. Both run on the nine real lives of `tests/fixtures/lives`
//! through the production readers: a `Job` with a live-shaped JSON state
//! (levels from XP, pack, bank, worn, coins), `ready_ctx`, `live_levels`,
//! `free_coins`, `claimed_by_others`; nothing the production does not fill.

use std::sync::Arc;

use lifescape_json::{object, str_value, Value};
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::BotState;
use lifescape_planner::guide_model::{Confidence, Family, FlowLine, MoneyRow, Outlet, PriceBasis, Prereqs, Quote};
use lifescape_planner::guide_read::{MoneyView, TrainingView};
use lifescape_planner::{Life, Node, NodeKind};

use crate::guides_store::{band_end, GuideSet};
use crate::jobs::Job;

fn line(item: &str, cause: &str, per_6000: f64, price: f64) -> FlowLine {
    FlowLine { item: item.to_string(), cause: cause.to_string(), per_6000, unit_price: price, value_6000: per_6000 * price, outlet: Outlet::Market, traded: true, basis: PriceBasis::Traded }
}

fn row(method: &str, product: &str, inputs: &[&str], level: u8) -> MoneyRow {
    let mut flows = vec![line(product, "product", 100.0, 40.0)];
    flows.extend(inputs.iter().map(|input| line(input, "input", -100.0, 10.0)));
    MoneyRow {
        family: Family::Market,
        method: method.to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 26,
        prereqs: Prereqs { levels: vec![("smithing".to_string(), level)], ..Prereqs::default() },
        flows,
        other_drops: Default::default(),
        revenue_6000: 4000.0,
        cost_6000: 2200.0,
        profit_6000: 1800.0 + f64::from(level),
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        priced: true,
    }
}

/// A set whose market rows serve every band, quotes for each object.
fn guide_set() -> GuideSet {
    let rows = vec![row("smelt:bronze_bar", "bronze_bar", &["copper_ore", "tin_ore"], 1), row("smelt:iron_bar", "iron_bar", &["iron_ore"], 15), row("smelt:steel_bar", "steel_bar", &["iron_ore", "coal"], 30)];
    let view = |level| MoneyView { level: Some(level), rows: rows.clone(), gaps: Vec::new(), recipes: Vec::new() };
    let mut bands: Vec<(u8, MoneyView)> = Vec::new();
    for level in 1..=99u8 {
        let end = band_end(level);
        if !bands.iter().any(|(known, _)| *known == end) {
            bands.push((end, view(level)));
        }
    }
    let mut quotes = std::collections::HashMap::new();
    for (item, asks) in [("copper_ore", 60.0), ("tin_ore", 200.0), ("iron_ore", 90.0), ("coal", 400.0)] {
        quotes.insert(item.to_string(), Quote { price: 10.0, traded: true, asks: vec![(10.0, asks)], ..Quote::default() });
    }
    for item in ["bronze_bar", "iron_bar", "steel_bar"] {
        quotes.insert(item.to_string(), Quote { price: 40.0, traded: true, median: 40.0, fills: 5, fill_units: 150.0, ..Quote::default() });
    }
    GuideSet {
        built_unix_ms: 0,
        built: std::time::Instant::now(),
        cycle_ms: 0,
        cycle_cpu_ms: None,
        market_wait_ticks: None,
        money_catalog: view(30),
        training_catalog: TrainingView { level: None, rows: Vec::new(), gaps: Vec::new() },
        money_bands: bands,
        training_bands: Vec::new(),
        quotes,
        stats: Value::Null,
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
}

fn stack(name: &str, count: i64) -> Option<Value> {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next()?;
    Some(object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))]))
}

/// A live-shaped state: every skill at a random XP, the objects of the
/// rows in pack / bank / worn in random counts, coins.
fn live_state(rng: &mut Rng) -> Value {
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(*[0i64, 400, 5_000, 60_000, 900_000, 6_000_000].get(rng.below(6) as usize).unwrap_or(&0))), ("level", Value::Int(1))])).collect();
    let (mut inv, mut bank, mut worn) = (Vec::new(), Vec::new(), Vec::new());
    for name in ["copper_ore", "tin_ore", "iron_ore", "coal", "bronze_bar", "iron_bar", "steel_bar", "hammer"] {
        let units = *[0i64, 1, 2, 7, 30, 140].get(rng.below(6) as usize).unwrap_or(&0);
        if units == 0 {
            continue;
        }
        if let Some(entry) = stack(name, units) {
            match rng.below(5) {
                0 | 1 => bank.push(entry),
                2 if units == 1 => worn.push(entry),
                _ => inv.push(entry),
            }
        }
    }
    let coins = *[0i64, 50, 2_000, 500_000, 5_000_000].get(rng.below(5) as usize).unwrap_or(&0);
    if let Some(entry) = stack("coins", coins) {
        inv.push(entry);
    }
    object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", Value::Arr(inv)),
        ("bank", Value::Arr(bank)),
        ("worn", Value::Arr(worn)),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(true)),
    ])
}

/// The body of `publish_on` as it stood before lot 1.1: `holdings_count`
/// per object (a copy of the live state and the locks read each time).
fn publish_reference(set: &GuideSet, ctx: &mut ReadyCtx, node: &Node, live: &Value, job: &Job) {
    let levels = crate::brain_market::live_levels(live);
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    let held = |item: &str| crate::brain_plan::holdings_count(job, item) as f64;
    let asks = |item: &str| crate::brain_trade::cycle_asks(set, item);
    let wallet = crate::brain_gear::free_coins(job, None);
    let bot = BotState { level_of: &level_of, combat: ctx.combat, quests_done: &ctx.quests_done, held: &held, coins: Some(wallet as f64), ironman: false, asks: &asks };
    let claims = crate::market_claims::claimed_by_others(&job.account);
    let claimed = |item: &str| claims.get(item).copied().unwrap_or(0) as f64;
    ctx.trades = crate::brain_trade::offers_for(set, node, &bot, &claimed);
    ctx.trade_wallet = wallet;
}

/// Money / coin nodes of the life, else a plain money node; each also
/// with the "bought for" lot that left the guide (`blocked_by`).
fn nodes_of(life: &Life) -> Vec<Node> {
    let mut nodes: Vec<Node> = life.nodes.iter().filter(|node| matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. })).take(3).cloned().collect();
    if nodes.is_empty() {
        nodes.push(Node::new("n9003", NodeKind::MakeMoney, Vec::new(), 1));
    }
    let mut bought = nodes[0].clone();
    bought.blocked_by = Some("trade:craft/bronze_bar".to_string());
    nodes.push(bought);
    nodes
}

#[test]
fn trades_are_identical_on_the_real_lives_through_the_production_readers() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let lives = crate::brain_launch_equiv_tests::load_dir(&dir);
    assert!(lives.len() >= 9, "fixtures missing");
    let set = guide_set();
    let (mut scenarios, mut offered, mut wallets) = (0usize, 0usize, std::collections::BTreeSet::new());
    for life in &lives {
        for node in nodes_of(life) {
            for variant in 0..40u64 {
                let mut rng = Rng(0x7ADE ^ variant.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (scenarios as u64) << 16);
                rng.next();
                let live = live_state(&mut rng);
                let job = Arc::new(Job::new(&format!("{}trade_{}", crate::brain_commerce::PROBE_PREFIX, life.account)));
                job.inner.lock().expect("job").live = Some(live.clone());
                let (mut before, mut after) = (crate::brain_plan::ready_ctx(life, &node, &live), crate::brain_plan::ready_ctx(life, &node, &live));
                publish_reference(&set, &mut before, &node, &live, &job);
                crate::brain_trade::publish_on(&set, &mut after, &node, &live, &job);
                assert_eq!(before.trades, after.trades, "{} {} variant {variant}", life.account, node.id);
                assert_eq!(before.trade_wallet, after.trade_wallet, "{} {} variant {variant}", life.account, node.id);
                scenarios += 1;
                offered += usize::from(!after.trades.is_empty());
                wallets.insert(after.trade_wallet);
            }
        }
    }
    eprintln!("trade.publish equivalence: {scenarios} scenarios identical, {offered} with offers, {} distinct wallets", wallets.len());
    assert!(offered > scenarios / 10, "the proof must reach offers: {offered}/{scenarios}");
}
