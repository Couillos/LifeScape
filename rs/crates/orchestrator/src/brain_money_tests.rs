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
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
    assert_eq!(methods(&lines), vec![("fight:melee/cows", true), ("gather:fishing/raw_shrimp", false)]);
    ctx.trades = vec![TradeOffer { method: "trade:craft/smelt:bronze_bar".to_string(), product: "bronze_bar".to_string(), profit_6000: 1100.0, product_per_6000: 100.0, unit_sale: 30.0, inputs: Vec::new(), demand: 100.0, unit_cost: 19.0, sale: Vec::new(), craft_ticks_per_unit: 60.0, purchase_ticks: 0.0 }];
    let stock = vec![("iron_platebody".to_string(), 40.0)];
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &stock, 0.5, &|_| None, &guide_profit));
    assert_eq!(methods(&lines), vec![("alch:iron_platebody", true), ("trade:craft/smelt:bronze_bar", true), ("fight:melee/cows", true), ("gather:fishing/raw_shrimp", false)]);
    assert_eq!(lines[1].profit_6000, 1100.0, "a trade lot ranks on its offer");
    assert_eq!(lines[2].runs, 4, "the yield's measured runs ride along (D14)");
    // Platebodies it keeps (gear) are no stock: never alched.
    let worn = |item: &str| if item == "iron_platebody" { 40.0 } else { 0.0 };
    let kept = judge(&worn, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
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
    ctx.trades = vec![TradeOffer { method: "trade:craft/smelt:bronze_bar".to_string(), product: "bronze_bar".to_string(), profit_6000: 1200.0, product_per_6000: 100.0, unit_sale: 30.0, inputs: Vec::new(), demand: 100.0, unit_cost: 19.0, sale: Vec::new(), craft_ticks_per_unit: 60.0, purchase_ticks: 0.0 }];
    let none = |_: &str| 0.0;
    let trader = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 1.0, &|_| None, &guide_profit));
    let hermit = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.0, &|_| None, &guide_profit));
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

/// R7, recipes on stock beyond alchemy (2026-11-17): a bot holding 200
/// raw trout reads "cook:trout on its stock" as a money line it works
/// now, its lot the 200 trout; a pair recipe is bound by its scarcer ore;
/// without the stock (or kept out of it) no line.
#[test]
fn a_recipe_on_its_own_stock_is_a_money_line_with_its_lot() {
    let mut trout = row(Family::Recipe, "cook:trout", 3000.0);
    trout.prereqs.stock = vec![("raw_trout".to_string(), 28.0)];
    let mut bronze = row(Family::Recipe, "smelt:bronze_bar", 900.0);
    bronze.prereqs.stock = vec![("copper_ore".to_string(), 14.0), ("tin_ore".to_string(), 14.0)];
    let owned = [trout, bronze];
    let rows: Vec<&MoneyRow> = owned.iter().collect();
    let ctx = ReadyCtx::blind(0.5);
    let none = |_: &str| 0.0;
    let stock = vec![("raw_trout".to_string(), 200.0), ("copper_ore".to_string(), 200.0), ("tin_ore".to_string(), 50.0)];
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &stock, 0.5, &|_| None, &guide_profit));
    assert_eq!(methods(&lines), vec![("cook:trout", true), ("smelt:bronze_bar", true)]);
    assert_eq!((lines[0].lot, lines[0].family.as_str()), (200, "recipe"));
    assert_eq!(lines[1].lot, 50, "the tin binds the bronze");
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
    assert!(lines.is_empty(), "no stock, no recipe line: {:?}", methods(&lines));
    assert!(super::stock_recipe(&owned[0]) && super::stock_recipe(&owned[1]), "every launchable recipe, not only alchemy");
}

/// A money node's own candidates (the alternatives a previous resolve
/// left: cook:trout) never reserve its stock against itself; another
/// node's plan still does (a cooking climb keeps its raw fish).
#[test]
fn a_money_node_reads_its_stock_without_its_own_candidates() {
    let mut life = crate::brain_sales::tests::life_training(&[("cooking", "cook:trout")]);
    let mut money = lifescape_planner::Node::new("n9", lifescape_planner::NodeKind::MakeMoney, Vec::new(), 0);
    money.method = Some("cook:lobster".to_string());
    money.alternatives = vec![lifescape_planner::Alternative { method: "cook:lobster".to_string(), est_ticks: 0, est_coins: 0 }];
    life.nodes.push(money);
    let all = crate::brain_sales::working_keep(&life, &[]);
    assert!(all.contains(&"raw_lobster".to_string()) && all.contains(&"raw_trout".to_string()), "{all:?}");
    let for_money = crate::brain_sales::working_keep_except(&life, &[], Some("n9"));
    assert!(!for_money.contains(&"raw_lobster".to_string()), "its own candidate: {for_money:?}");
    assert!(for_money.contains(&"raw_trout".to_string()), "the climb's raw fish stays kept: {for_money:?}");
}

/// Point 2 (user 2026-11-18): nathan_t turned 3274 mithril bars into 564
/// full helms for a market that took one a day. A recipe's lot stops at
/// what its product's market absorbs; a product nobody absorbs is no
/// line at all (the bars stay bars for another use or buyer).
#[test]
fn a_stock_lot_stops_at_what_its_product_market_absorbs() {
    let mut trout = row(Family::Recipe, "cook:trout", 3000.0);
    trout.prereqs.stock = vec![("raw_trout".to_string(), 28.0)];
    let mut bronze = row(Family::Recipe, "smelt:bronze_bar", 900.0);
    bronze.prereqs.stock = vec![("copper_ore".to_string(), 14.0), ("tin_ore".to_string(), 14.0)];
    let owned = [trout, bronze];
    let rows: Vec<&MoneyRow> = owned.iter().collect();
    let ctx = ReadyCtx::blind(0.5);
    let none = |_: &str| 0.0;
    let stock = vec![("raw_trout".to_string(), 200.0), ("copper_ore".to_string(), 200.0), ("tin_ore".to_string(), 50.0)];
    let room = |method: &str| Some(if method == "cook:trout" { 40 } else { 0 });
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &stock, 0.5, &room, &guide_profit));
    assert_eq!(methods(&lines), vec![("cook:trout", true)], "no room for bronze bars: no line");
    assert_eq!(lines[0].lot, 40, "200 trout held, 40 cooked the market takes");
}

/// The guide's own profit of a row (no claim read).
fn guide_profit(row: &MoneyRow) -> f64 {
    row.profit_6000
}

/// Coal mined 100 a window, sold at 15: the last fills moved 100 coal.
fn coal_row() -> MoneyRow {
    let flow = lifescape_planner::guide_model::FlowLine {
        item: "coal".to_string(),
        cause: "product".to_string(),
        per_6000: 100.0,
        unit_price: 15.0,
        value_6000: 1500.0,
        outlet: lifescape_planner::guide_model::Outlet::Market,
        traded: true,
        basis: lifescape_planner::guide_model::PriceBasis::Traded,
    };
    MoneyRow { flows: vec![flow], ..row(Family::Farm, "gather:mining/coal", 1500.0) }
}

/// Coal as the cycle quoted it, `claimed` units by the other producers.
fn coal_quote(item: &str, claimed: f64) -> Option<lifescape_planner::guide_model::Quote> {
    if item != "coal" {
        return None;
    }
    Some(lifescape_planner::guide_model::Quote { price: 15.0, traded: true, median: 15.0, fills: 5, fill_units: 100.0, claimed, ..Default::default() })
}

/// The lines of the coal and cow rows for a bot reading `claimed` coal
/// claimed by the others.
fn coal_lines(claimed: f64) -> Vec<lifescape_planner::guide_pick::GuideLine> {
    let owned = [coal_row(), row(Family::Farm, "fight:melee/cows", 800.0)];
    let rows: Vec<&MoneyRow> = owned.iter().collect();
    let ctx = ReadyCtx::blind(0.5);
    let none = |_: &str| 0.0;
    let no_shop = |_: &str| None;
    let quote = |item: &str| coal_quote(item, claimed);
    let demand = lifescape_planner::guide_share::SharedDemand::new(&quote, &no_shop);
    let shared = |row: &MoneyRow| demand.profit(row);
    judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &shared))
}

/// Plan équilibre step 2: two bots choose one after the other. The
/// first reads coal at the guide's 1500 and ranks it first; once it
/// claims its 100 coal on the public board, the second reads the coal
/// row on the demand left (none: the line goes) and takes the cows.
#[test]
fn a_claim_lowers_another_bots_line_at_its_pick() {
    let first = coal_lines(0.0);
    assert_eq!(methods(&first), vec![("gather:mining/coal", true), ("fight:melee/cows", true)]);
    assert!((first[0].profit_6000 - 1500.0).abs() < 1e-6);
    let half = coal_lines(40.0);
    assert!((half[0].profit_6000 - 60.0 * 15.0).abs() < 1e-6, "40 claimed: 60 coal left at 15: {:?}", half[0]);
    let second = coal_lines(100.0);
    assert_eq!(methods(&second), vec![("fight:melee/cows", true)], "the first bot's 100 coal took the demand");
}

/// What a launched money run posts on the order board: a farm method
/// its measured units over its budget, a trade lot its lot, a recipe on
/// its stock its lot of actions; alchemy nothing.
#[test]
fn a_money_run_claims_what_it_brings() {
    let mut node = lifescape_planner::Node::new("n4", lifescape_planner::NodeKind::MakeMoney, Vec::new(), 0);
    let rate = |_: &str, _: &str| Some(120.0);
    let unmeasured = |_: &str, _: &str| None;
    assert_eq!(super::run_claim(&node, "gather:mining/coal", 18_000, &rate), Some(("coal".to_string(), 360)));
    assert_eq!(super::run_claim(&node, "gather:mining/coal", 18_000, &unmeasured), None, "no measured rate: no claim");
    assert_eq!(super::run_claim(&node, "alch:iron_platebody", 6_000, &rate), None, "alchemy brings coins");
    node.batch = 15;
    assert_eq!(super::run_claim(&node, "trade:craft/smelt:bronze_bar", 6_000, &rate), Some(("bronze_bar".to_string(), 15)));
    // A recipe on its stock: 15 actions × the trout an action yields
    // (the burns counted).
    let (_, way) = lifescape_planner::method_way::method_way("cook:trout").expect("cooking trout");
    let cooked = (15.0 * way.expected_out()).floor() as i64;
    assert!(cooked > 0 && cooked <= 15, "{cooked}");
    assert_eq!(super::run_claim(&node, "cook:trout", 6_000, &rate), Some(("trout".to_string(), cooked)));
    // Review lot 2, point 6: a batch left by an earlier stock lot never
    // sizes a harvest: its units are its measured rate over the budget.
    assert_eq!(super::run_claim(&node, "gather:mining/coal", 18_000, &rate), Some(("coal".to_string(), 360)));
}

/// Review lot 3, point 3: a filler window cooking its 200 raw trout
/// stops after 1500 ticks: it claims what the window makes at its
/// measured rate (120 a window of 6000: 30), never the whole lot.
#[test]
fn a_filler_stock_lot_claims_what_its_window_makes() {
    let mut filler = lifescape_planner::Node::new("fill-n7", lifescape_planner::NodeKind::HaveCoins { amount: 1 }, Vec::new(), 0);
    filler.batch = 200;
    let rate = |_: &str, _: &str| Some(120.0);
    assert_eq!(super::run_claim(&filler, "cook:trout", 1_500, &rate), Some(("trout".to_string(), 30)));
    filler.batch = 10;
    let (_, way) = lifescape_planner::method_way::method_way("cook:trout").expect("cooking trout");
    let small = (10.0 * way.expected_out()).floor() as i64;
    assert_eq!(super::run_claim(&filler, "cook:trout", 1_500, &rate), Some(("trout".to_string(), small)), "a small lot: the lot");
}

/// Review lot 2, point 5: a trade line ranks on the lot the bot really
/// takes (demand, wallet, asks), its purchase spread over that lot only.
#[test]
fn a_trade_line_ranks_on_the_lot_the_bot_takes() {
    let owned = [row(Family::Market, "smelt:bronze_bar", 1200.0)];
    let rows: Vec<&MoneyRow> = owned.iter().collect();
    let mut ctx = ReadyCtx::blind(0.5);
    let input = lifescape_planner::trade_craft::TradeInput { item: "copper_ore".to_string(), per_unit: 1.0, asks: vec![(10.0, 1000.0)], floor: 0.0, held: 0 };
    let sale = vec![lifescape_planner::guide_engine::SaleLevel { price: 40.0, depth: 10.0, market: true }];
    let offer = TradeOffer { method: "trade:craft/smelt:bronze_bar".to_string(), product: "bronze_bar".to_string(), profit_6000: 1800.0, product_per_6000: 100.0, unit_sale: 40.0, inputs: vec![input], demand: 10.0, unit_cost: 10.0, sale, craft_ticks_per_unit: 50.0, purchase_ticks: 500.0 };
    ctx.trades = vec![offer.clone()];
    ctx.trade_wallet = 1_000_000;
    let none = |_: &str| 0.0;
    let lines = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
    // 10 bars (the demand) net 300 in 10 × 50 + 500 ticks.
    let expected = 300.0 * 6000.0 / 1000.0;
    assert!((lines[0].profit_6000 - expected).abs() < 1e-6, "{} vs {expected}", lines[0].profit_6000);
    // A wallet of 50 coins: 5 bars, 150 in 750 ticks.
    ctx.trade_wallet = 50;
    let poor = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
    assert!((poor[0].profit_6000 - 150.0 * 6000.0 / 750.0).abs() < 1e-6, "{}", poor[0].profit_6000);
}

/// Live 2026-11-22: ~10 bots picked the same waterrune spawn as a filler
/// (517 runs a day) because the line read the whole spawn's rate. A ground
/// line is worth the share one more bot gets (rate over bots there + 1).
#[test]
fn a_crowded_ground_spawn_line_is_shared() {
    let rows = [row(Family::Farm, "ground:waterrune", 2400.0)];
    let rows: Vec<&MoneyRow> = rows.iter().collect();
    let sites: Vec<(i64, i64, i64)> = lifescape_planner::method_way::method_way("ground:waterrune").expect("way").1.sites;
    let mut ctx = ReadyCtx::blind(0.5);
    let none = |_: &str| 0.0;
    let alone = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
    assert_eq!(alone.first().map(|line| line.profit_6000), Some(2400.0), "empty spawn: whole rate");
    ctx.seats = sites.iter().map(|site| (*site, "ground:waterrune".to_string(), 10)).collect();
    let crowded = judge(&none, |bot| lines_for(&rows, &ctx, bot, &[], 0.5, &|_| None, &guide_profit));
    let profit = crowded.first().map(|line| line.profit_6000).expect("still a line");
    assert!((profit - 2400.0 / 11.0).abs() < 1e-6, "10 bots on every tile: a eleventh, got {profit}");
}
