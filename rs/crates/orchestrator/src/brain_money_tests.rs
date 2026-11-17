//! The money guide's lines a bot may take (R7): fabricated rows of a
//! precomputed set, judged on a fabricated live bot.

use std::collections::HashMap;
use std::time::Instant;

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::BotState;
use lifescape_planner::guide_model::{Confidence, Family, MoneyRow, Prereqs};
use lifescape_planner::guide_read::{MoneyView, TrainingView};
use lifescape_planner::trade_craft::TradeOffer;

use super::{bot_rows, lines_for};
use crate::guides_store::GuideSet;

fn row(family: Family, method: &str, profit: f64) -> MoneyRow {
    MoneyRow {
        family,
        method: method.to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 26,
        prereqs: Prereqs { ironman: family != Family::Market, ..Prereqs::default() },
        flows: Vec::new(),
        other_drops: Default::default(),
        revenue_6000: profit,
        cost_6000: 0.0,
        profit_6000: profit,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence { runs: 4, ..Confidence::default() },
        price_confidence: "traded",
        priced: true,
    }
}

/// A level-30 bot, 500 free coins, `held` objects.
fn judge<T>(held: &dyn Fn(&str) -> f64, body: impl FnOnce(&BotState) -> T) -> T {
    let level_of = |_: &str| 30u8;
    let asks = |_: &str| None;
    let bot = BotState { level_of: &level_of, combat: 40, quests_done: &[], held, coins: Some(500.0), ironman: false, asks: &asks };
    body(&bot)
}

fn methods(lines: &[lifescape_planner::guide_pick::GuideLine]) -> Vec<(&str, bool)> {
    lines.iter().map(|line| (line.method.as_str(), line.feasible)).collect()
}

/// Every family maps to what a money node launches: a farm row its own
/// method, a market row its trade lot when the bot has the offer, a
/// recipe on its stock its `alch:` method; a chain, an unpriced or a
/// losing row, a row out of reach never; a tool to buy keeps the line,
/// not feasible yet.
#[test]
fn rows_map_to_the_methods_a_money_node_launches() {
    let mut yews = row(Family::Farm, "gather:woodcut/yew_logs", 5000.0);
    yews.prereqs.levels = vec![("woodcutting".to_string(), 60)];
    let mut shrimps = row(Family::Farm, "gather:fishing/raw_shrimp", 700.0);
    shrimps.prereqs.tools = vec!["net".to_string()];
    let mut unpriced = row(Family::Farm, "gather:sheep/wool", 900.0);
    unpriced.priced = false;
    let mut alch = row(Family::Recipe, "alch:iron_platebody", 1500.0);
    alch.prereqs.stock = vec![("iron_platebody".to_string(), 28.0)];
    let rows = vec![
        yews,
        row(Family::Farm, "fight:melee/cows", 800.0),
        row(Family::Farm, "fight:melee/chickens", -5.0),
        shrimps,
        unpriced,
        row(Family::Market, "smelt:bronze_bar", 1200.0),
        row(Family::Chain, "smith:bronze_scimitar", 3000.0),
        alch,
    ];
    let mut ctx = ReadyCtx::blind(0.5);
    let none = |_: &str| 0.0;
    let rows: Vec<&MoneyRow> = rows.iter().collect();
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5));
    assert_eq!(methods(&lines), vec![("fight:melee/cows", true), ("gather:fishing/raw_shrimp", false)]);
    ctx.trades = vec![TradeOffer { method: "trade:craft/smelt:bronze_bar".to_string(), product: "bronze_bar".to_string(), profit_6000: 1100.0, product_per_6000: 100.0, unit_sale: 30.0, inputs: Vec::new() }];
    let stock = vec![("iron_platebody".to_string(), 40.0)];
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &stock, 0.5));
    assert_eq!(methods(&lines), vec![("alch:iron_platebody", true), ("trade:craft/smelt:bronze_bar", true), ("fight:melee/cows", true), ("gather:fishing/raw_shrimp", false)]);
    assert_eq!(lines[1].profit_6000, 1100.0, "a trade lot ranks on its offer");
    assert_eq!(lines[2].runs, 4, "the yield's measured runs ride along (D14)");
    // Platebodies it keeps (gear) are no stock: never alched.
    let worn = |item: &str| if item == "iron_platebody" { 40.0 } else { 0.0 };
    let kept = judge(&worn, |bot| lines_for(&rows, &ctx, bot, &[], 0.5));
    assert!(!kept.iter().any(|line| line.method == "alch:iron_platebody"), "{:?}", methods(&kept));
}

/// The profile weighs the draw (D13): a market line counts double for a
/// market bot, two thirds for an autarkic one; the score stays the
/// guide's profit.
#[test]
fn market_affinity_weighs_a_market_line() {
    let owned = [row(Family::Market, "smelt:bronze_bar", 1200.0)];
    let rows: Vec<&MoneyRow> = owned.iter().collect();
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.trades = vec![TradeOffer { method: "trade:craft/smelt:bronze_bar".to_string(), product: "bronze_bar".to_string(), profit_6000: 1200.0, product_per_6000: 100.0, unit_sale: 30.0, inputs: Vec::new() }];
    let none = |_: &str| 0.0;
    let trader = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 1.0));
    let hermit = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.0));
    assert!((trader[0].weight - 2400.0).abs() < 1e-6 && (hermit[0].weight - 800.0).abs() < 1e-6);
    assert_eq!(trader[0].profit_6000, hermit[0].profit_6000);
}

fn set_with(bands: Vec<(u8, Vec<MoneyRow>)>) -> GuideSet {
    GuideSet {
        built_unix_ms: 0,
        built: Instant::now(),
        cycle_ms: 0,
        cycle_cpu_ms: None,
        market_wait_ticks: None,
        money_catalog: MoneyView::default(),
        training_catalog: TrainingView::default(),
        money_bands: bands.into_iter().map(|(end, rows)| (end, MoneyView { level: Some(end), rows, ..MoneyView::default() })).collect(),
        training_bands: Vec::new(),
        quotes: HashMap::new(),
        stats: lifescape_json::Value::Null,
    }
}

/// Each activity is read in the view of the bot's level in its band
/// skill (woodcutting 60: the 56-60 view's yews), the rest at their own.
#[test]
fn each_activity_reads_the_view_of_the_bots_level_in_its_skill() {
    let set = set_with(vec![
        (30, vec![row(Family::Farm, "gather:woodcut/yew_logs", 100.0), row(Family::Farm, "gather:sheep/wool", 50.0)]),
        (60, vec![row(Family::Farm, "gather:woodcut/yew_logs", 900.0), row(Family::Farm, "gather:sheep/wool", 50.0)]),
    ]);
    let level_of = |skill: &str| if skill == "woodcutting" { 60 } else { 30 };
    let read = bot_rows(&set, &level_of, &[]);
    assert!(read.errors.is_empty());
    let rows = read.all();
    let yews: Vec<f64> = rows.iter().filter(|row| row.method == "gather:woodcut/yew_logs").map(|row| row.profit_6000).collect();
    assert_eq!(yews, vec![900.0], "the woodcutter's own band");
    assert_eq!(rows.iter().filter(|row| row.method == "gather:sheep/wool").count(), 1, "a row without a band skill once");
}
