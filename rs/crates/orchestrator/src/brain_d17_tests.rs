//! The money guide as the one source of the brain's coin figures
//! (decision 2026-11-17): the D17 of a bot, its coin rate, a method's
//! profit and the ambition kinds' profits all read the precomputed set.

use std::collections::HashMap;
use std::time::Instant;

use lifescape_planner::guide_model::{Confidence, Family, GuideFilter, MoneyRow, Prereqs};
use lifescape_planner::guide_read::{MoneyView, TrainingView};

use super::{best_row, kind_profits, method_profit_in, BotKey};
use crate::guides_store::GuideSet;

/// A bot's measured time value (D1), distinct from any D17 of the tests.
const MEASURED: f64 = 0.37;

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

fn set_with(catalog: Vec<MoneyRow>, bands: Vec<(u8, Vec<MoneyRow>)>) -> GuideSet {
    GuideSet {
        built_unix_ms: 0,
        built: Instant::now(),
        cycle_ms: 0,
        cycle_cpu_ms: None,
        market_wait_ticks: None,
        money_catalog: MoneyView { rows: catalog, ..MoneyView::default() },
        training_catalog: TrainingView::default(),
        money_bands: bands.into_iter().map(|(end, rows)| (end, MoneyView { level: Some(end), rows, ..MoneyView::default() })).collect(),
        training_bands: Vec::new(),
        quotes: HashMap::new(),
        stats: lifescape_json::Value::Null,
    }
}

/// Combat of a bot at `level` in every skill, as `guide_feasible::
/// with_filter_bot` reads it for a filter.
/// The D17 price of a bot: the profit of its D17 line (`best_row`).
fn bot_d17(set: &GuideSet, key: &BotKey) -> Option<f64> {
    best_row(set, key).map(|row| row.profit_6000)
}

fn combat_at(level: u8) -> u8 {
    lifescape_planner::combat::combat_level(&lifescape_planner::combat::BaseLevels { attack: level, strength: level, defence: level, hitpoints: level.max(10), ranged: level, prayer: level, magic: level })
}

/// A bot at one level in every skill reads, in the set built from the
/// game priors, exactly the D17 of R7 (`brain_train`'s: the money view of
/// its band read by `guide_read::best_profit_in`), with and without a
/// quest.
#[test]
fn a_uniform_bot_reads_the_d17_of_r7() {
    let store = lifescape_store::Store::open_in_memory().expect("store");
    let set = crate::guides_store::build_set(&store, &crate::rates_service::Snapshot::new(), &crate::web_guides::StoreWaits::default());
    let mut compared = 0;
    for quests in [Vec::new(), vec!["doric".to_string()]] {
        for level in [1u8, 5, 12, 30, 47, 65, 80, 99] {
            let level_of = |_: &str| level;
            let key = BotKey { level_of: &level_of, combat: combat_at(level), quests_done: &quests, ironman: false };
            let r7 = lifescape_planner::guide_read::best_profit_in(set.money_view(Some(level)), &GuideFilter { level: Some(level), quests_done: quests.clone(), ..GuideFilter::default() });
            assert_eq!(bot_d17(&set, &key), r7, "level {level}, quests {quests:?}");
            compared += usize::from(r7.is_some());
        }
    }
    assert!(compared > 0, "the priors price some line");
}

/// A bot's D17 is the best line within ITS reach: its level in each skill
/// opens its band's view, a quest it lacks, a level it lacks in another
/// skill, a stock it does not hold, an unpriced or losing row never
/// count; a tool to obtain does (within reach); an Ironman never counts a
/// market row.
#[test]
fn the_d17_is_the_best_line_within_the_bots_reach() {
    let mut quest = row(Family::Farm, "gather:woodcut/oak_logs", 9000.0);
    quest.prereqs.quests = vec!["doric".to_string()];
    let mut other_skill = row(Family::Farm, "gather:woodcut/willow_logs", 8000.0);
    other_skill.prereqs.levels = vec![("mining".to_string(), 40)];
    let mut stock = row(Family::Recipe, "alch:iron_platebody", 7000.0);
    stock.prereqs.stock = vec![("iron_platebody".to_string(), 28.0)];
    let mut unpriced = row(Family::Farm, "gather:sheep/wool", 6500.0);
    unpriced.priced = false;
    let mut tool = row(Family::Farm, "gather:fishing/raw_shrimp", 900.0);
    tool.prereqs.tools = vec!["net".to_string()];
    let market = row(Family::Market, "smelt:bronze_bar", 1200.0);
    let losing = row(Family::Farm, "fight:melee/chickens", -5.0);
    let set = set_with(Vec::new(), vec![(30, vec![quest, other_skill, stock, unpriced, tool, market, losing])]);
    let level_of = |skill: &str| if skill == "mining" { 1 } else { 30 };
    let none: Vec<String> = Vec::new();
    let bot = BotKey { level_of: &level_of, combat: 40, quests_done: &none, ironman: false };
    assert_eq!(bot_d17(&set, &bot), Some(1200.0), "the market row, the tool row within reach");
    let iron = BotKey { ironman: true, ..bot };
    assert_eq!(bot_d17(&set, &iron), Some(900.0), "an Ironman never counts a market row; a tool to obtain is within reach");
    let doric = vec!["doric".to_string()];
    let quester = BotKey { level_of: &level_of, combat: 40, quests_done: &doric, ironman: false };
    assert_eq!(bot_d17(&set, &quester), Some(9000.0), "its quest done");
    let low = |_: &str| 3u8;
    let novice = BotKey { level_of: &low, combat: 3, quests_done: &none, ironman: false };
    assert_eq!(bot_d17(&set, &novice), None, "no view of its band: no line");
}

/// A method's profit is its best priced row among the bot's rows (any
/// site), keyed by the activity it works; nothing priced, no profit.
#[test]
fn a_method_profit_is_its_guide_row() {
    let mut far = row(Family::Farm, "fight:melee/cows", 700.0);
    far.site = Some((3250, 3270, 0));
    let near = row(Family::Farm, "fight:melee/cows", 800.0);
    let mut unpriced = row(Family::Farm, "gather:sheep/wool", 900.0);
    unpriced.priced = false;
    let rows = [far, near, unpriced];
    let refs: Vec<&MoneyRow> = rows.iter().collect();
    assert_eq!(method_profit_in(&refs, "fight:melee/cows"), Some(800.0));
    assert_eq!(method_profit_in(&refs, "gather:sheep/wool"), None, "unpriced");
    assert_eq!(method_profit_in(&refs, "gather:woodcut/logs"), None, "not in the guide");
}

/// A node reads its method profits in the guide (`brain_market::
/// guide_coins`, what `fill_ctx` runs on the published set): a money
/// node's methods carry their guide profit (the willow glut the guide
/// prices low, D12, never a saturation on top), and the static ranking
/// then takes cows (old live case 2026-11-14: willow published at 16 732
/// / 6000 with 33 364 logs unsold at 1 coin). The grind's coin rate is
/// NOT the guide's (D1, plan équilibre step 1): `ready_ctx` set it to the
/// bot's measured time value, the guide leaves it as it found it.
#[test]
fn a_node_reads_its_coin_figures_in_the_guide() {
    let rows = vec![row(Family::Farm, "gather:woodcut/willow_logs", 1200.0), row(Family::Farm, "fight:melee/cows", 3000.0)];
    let set = set_with(Vec::new(), vec![(5, rows.clone()), (20, rows.clone()), (35, rows)]);
    let mut ctx = lifescape_planner::expand::ReadyCtx::blind(0.5);
    ctx.coins_per_tick = MEASURED;
    ctx.levels = vec![("woodcutting".to_string(), 35), ("attack".to_string(), 20), ("strength".to_string(), 20), ("defence".to_string(), 20), ("hitpoints".to_string(), 20)];
    ctx.tools = vec!["bronze_axe".to_string()];
    let mut money = lifescape_planner::Node::new("n1", lifescape_planner::NodeKind::MakeMoney, Vec::new(), 0);
    crate::brain_market::guide_coins(&mut ctx, &money, &set);
    assert_eq!(ctx.coins_per_tick, MEASURED, "the measured time value, never the D17 (3000 / 6000)");
    let profit = |method: &str| ctx.method_profit.iter().find(|(known, _)| known == method).map(|(_, profit)| *profit);
    assert_eq!(profit("gather:woodcut/willow_logs"), Some(1200.0), "{:?}", ctx.method_profit);
    assert_eq!(profit("fight:melee/cows"), Some(3000.0), "{:?}", ctx.method_profit);
    lifescape_planner::resolve_ready(&mut money, &ctx);
    assert_eq!(money.method.as_deref(), Some("fight:melee/cows"), "{:?}", money.alternatives);
    // An obtain node prices its grind at the same measured value, no
    // method profit.
    let mut obtain_ctx = lifescape_planner::expand::ReadyCtx::blind(0.5);
    obtain_ctx.coins_per_tick = MEASURED;
    obtain_ctx.levels = ctx.levels.clone();
    let obtain = lifescape_planner::Node::new("n2", lifescape_planner::NodeKind::ObtainItems { item: "iron_bar".to_string(), count: 5 }, Vec::new(), 5);
    crate::brain_market::guide_coins(&mut obtain_ctx, &obtain, &set);
    assert_eq!(obtain_ctx.coins_per_tick, MEASURED);
    assert!(obtain_ctx.method_profit.is_empty());
}

/// The ambition kinds weigh the guide's catalogue: the best priced row of
/// each kind (by its band skill), never a row impossible for every bot.
#[test]
fn kind_profits_aggregate_the_catalogue_by_kind() {
    let mut impossible = row(Family::Chain, "smith:rune_axe", 90_000.0);
    impossible.prereqs.combat = lifescape_gamedata::danger::NEVER;
    let mut unpriced = row(Family::Farm, "gather:mining/iron_ore", 50_000.0);
    unpriced.priced = false;
    let catalog = vec![
        row(Family::Farm, "gather:woodcut/willow_logs", 1500.0),
        row(Family::Farm, "gather:woodcut/oak_logs", 900.0),
        row(Family::Farm, "gather:mining/copper_ore", 400.0),
        impossible,
        unpriced,
    ];
    let set = set_with(catalog, Vec::new());
    let profits = kind_profits(&set);
    let kind = |skill: &str| lifescape_planner::ambition_catalog::kind_for_skill(skill).expect("kind").to_string();
    let of = |name: &str| profits.iter().find(|(known, _)| *known == name).map(|(_, profit)| *profit);
    assert_eq!(of(&kind("woodcutting")), Some(1500.0), "{profits:?}");
    if kind("mining") != kind("woodcutting") {
        assert_eq!(of(&kind("mining")), Some(400.0), "unpriced iron never counts: {profits:?}");
    }
    let smith = kind("smithing");
    if smith != kind("woodcutting") && smith != kind("mining") {
        assert_eq!(of(&smith), None, "an impossible chain never counts: {profits:?}");
    }
}

/// Live 2026-11-19: `alch:rune_platebody` (36.2 M a window on 1 887
/// plates of held stock nobody offers) was the mage's profit, 21 mages of
/// 54 in the D3 review replay. A kind's profit is the row's gain over
/// what the asks and the F2P shops really supply (`supplied_share`): 0
/// with no plate offered, 3/1 887 of it with three on the asks.
#[test]
fn kind_profits_count_only_the_stock_really_offered() {
    let casts = 1887.6;
    let mut alch = row(Family::Recipe, "alch:rune_platebody", 36_214_820.0);
    let line = |item: &str, cause: &str, per_6000: f64, unit_price: f64| lifescape_planner::guide_model::FlowLine {
        item: item.to_string(),
        cause: cause.to_string(),
        per_6000,
        unit_price,
        value_6000: per_6000 * unit_price,
        outlet: lifescape_planner::guide_model::Outlet::Market,
        traded: true,
        basis: lifescape_planner::guide_model::PriceBasis::Shop,
    };
    alch.flows = vec![line("rune_platebody", "input", -casts, 6500.0), line("coins", "product", 26_000.0 * casts, 1.0)];
    let mut set = set_with(vec![alch], Vec::new());
    let mage = |set: &GuideSet| kind_profits(set).into_iter().find(|(kind, _)| kind == "mage").map(|(_, profit)| profit).expect("mage");
    // The F2P shop window of the game data (Oziach's, after Dragon
    // Slayer: a handful a window), nothing on the asks.
    let shop: f64 = crate::web_guides::shop_window("rune_platebody").iter().map(|(_, units, _)| *units).sum();
    assert!(shop < 100.0, "a shop sells a handful at most: {shop}");
    let alone = mage(&set);
    assert!((alone - 36_214_820.0 * shop / casts).abs() < 1e-3, "only the shop's plates: {alone}");
    set.quotes.insert("rune_platebody".to_string(), lifescape_planner::guide_model::Quote { asks: vec![(30_000.0, 3.0)], ..Default::default() });
    let offered = mage(&set);
    assert!((offered - 36_214_820.0 * (shop + 3.0) / casts).abs() < 1e-3, "three more on the asks: {offered}");
    assert!(offered < 1_000_000.0, "never the 36 M window: {offered}");
}

/// One source of truth: no source file of the crate reads the old rates
/// profit table any more.
#[test]
fn no_reader_of_the_old_profit_table_is_left() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let needles = [["cached", "_profit_rows"].concat(), ["rates", "_profit"].concat(), ["fn ", "rate_rows"].concat(), ["profit", "_rows("].concat()];
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("src") {
        let path = entry.expect("entry").path();
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for needle in &needles {
            if text.contains(needle.as_str()) {
                found.push(format!("{}: {needle}", path.display()));
            }
        }
    }
    assert!(found.is_empty(), "{found:?}");
}
