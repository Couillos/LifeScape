//! Read side of the guides on fabricated worlds (`guide_read`): a view
//! read with a filter gives the guide built with it; a cyclic chain
//! ends; a yield is read once per build; a whole set of views stays in
//! a bounded time on the full F2P catalogue.

use std::cell::Cell;
use std::collections::HashMap;

use crate::guide_engine::Engine;
use crate::guide_model::{GuideFilter, GuideWorld, Quote, RateAnswer, RateAsk, ShopNeed};
use crate::guide_money::{money_guide, money_view};
use crate::guide_read::{finish_money, finish_training};
use crate::guide_training::{training_guide, training_view};

fn answer(flows: &[(&str, &str, f64)], units: &[(&str, &str, f64)]) -> RateAnswer {
    RateAnswer {
        flows: flows.iter().map(|(item, cause, qty)| (item.to_string(), cause.to_string(), *qty)).collect(),
        units: units.iter().map(|(kind, subject, qty)| (kind.to_string(), subject.to_string(), *qty)).collect(),
        pause_per_6000: 0.0,
        confidence: crate::guide_model::Confidence { measured: true, runs: 5, read: "Exact".to_string(), ..Default::default() },
    }
}

/// Coal, iron and steel; `cyclic`: mining iron spends a bar (a chain
/// whose upstream needs its own product).
fn yields(cyclic: bool) -> HashMap<&'static str, RateAnswer> {
    let mut out = HashMap::new();
    out.insert("gather:mining/coal", answer(&[("coal", "product", 100.0)], &[("product", "coal", 100.0)]));
    let iron: Vec<(&str, &str, f64)> = if cyclic { vec![("iron_ore", "product", 120.0), ("iron_bar", "input", -1.0)] } else { vec![("iron_ore", "product", 120.0)] };
    out.insert("gather:mining/iron_ore", answer(&iron, &[("product", "iron_ore", 120.0)]));
    out.insert("smelt:iron_bar", answer(&[("iron_bar", "product", 50.0), ("iron_ore", "input", -100.0)], &[("product", "iron_bar", 50.0)]));
    out.insert("smelt:steel_bar", answer(&[("steel_bar", "product", 100.0), ("iron_ore", "input", -100.0), ("coal", "input", -200.0)], &[("product", "steel_bar", 100.0)]));
    out.insert("fire:logs", answer(&[("logs", "input", -300.0)], &[("input", "logs", 300.0)]));
    out
}

fn traded(price: f64) -> Quote {
    Quote { price, traded: true, median: price, fills: 5, fill_units: 1e9, asks: vec![(price, 1e9)], ..Quote::default() }
}

/// Runs `body` on an engine over `yields` (a counter of the yields read).
fn with_engine<T>(yields: &HashMap<&'static str, RateAnswer>, reads: &Cell<usize>, body: impl FnOnce(&Engine) -> T) -> T {
    let prices = HashMap::from([("coal", 15.0), ("iron_ore", 10.0), ("iron_bar", 30.0), ("steel_bar", 50.0), ("logs", 4.0)]);
    let rate = |ask: &RateAsk| {
        reads.set(reads.get() + 1);
        yields.get(ask.method.as_str()).cloned().ok_or_else(|| format!("no yield for {}", ask.method))
    };
    let price = |item: &str| prices.get(item).map(|price| traded(*price)).unwrap_or(Quote { price: 1.0, ..Quote::default() });
    let npc = |_: &str| None;
    let shop = |_: &str| Vec::new();
    let quest = |_: &str| None;
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc, shop_buy: &shop, market_wait_ticks: Some(100.0), quest_ticks: &quest, measured: &[] };
    let engine = Engine::new(&world);
    body(&engine)
}

fn filter(level: Option<u8>) -> GuideFilter {
    GuideFilter { level, ..GuideFilter::default() }
}

/// A view read with a filter gives exactly the guide built with that
/// filter: skill, family, quests (accessibility), held (recipe rows
/// re-evaluated on the stock), money and training.
#[test]
fn a_view_read_with_a_filter_is_the_guide_built_with_it() {
    let world = yields(false);
    let reads = Cell::new(0);
    with_engine(&world, &reads, |engine| {
        let money = money_view(engine, Some(30), &[], &[]);
        let training = training_view(engine, Some(30), &[]);
        let skill = |name: &str| GuideFilter { skill: Some(name.to_string()), ..filter(Some(30)) };
        let family = GuideFilter { family: Some("market".to_string()), ..filter(Some(30)) };
        let quests = GuideFilter { quests_done: vec!["doric".to_string()], ..filter(Some(30)) };
        for read in [filter(Some(30)), skill("smithing"), skill("mining"), family.clone()] {
            assert_eq!(finish_money(&money, &read, &HashMap::new()), money_guide(engine, &read), "money {read:?}");
        }
        let bot = GuideFilter { quests_done: vec!["doric".to_string()], ..filter(Some(27)) };
        let copied = crate::guide_feasible::with_filter_bot(&bot, |judge| finish_money(&money, &bot, &HashMap::new()).0.iter().filter(|row| row.priced && row.profit_6000 > 0.0 && crate::guide_feasible::feasible_for(*row, judge).within_reach()).map(|row| row.profit_6000).reduce(f64::max));
        assert_eq!(crate::guide_read::best_profit_in(&money, &bot), copied, "D17 read without a copy");
        for read in [filter(Some(30)), skill("smithing"), skill("firemaking"), quests] {
            assert_eq!(finish_training(&training, &read, None), training_guide(engine, &read), "training {read:?}");
        }
        // A bot's stock: the recipe rows re-evaluated, the others kept.
        let held = vec![("iron_ore".to_string(), 50.0), ("coal".to_string(), 500.0)];
        let stocked = GuideFilter { held: held.clone(), ..filter(Some(30)) };
        let replaced: HashMap<usize, crate::guide_model::MoneyRow> = money.recipes.iter().map(|place| (place.row, crate::guide_money::recipe_row(engine, &place.ask, &place.answer, &held))).collect();
        assert!(!replaced.is_empty(), "the steel recipe has a place");
        assert_eq!(finish_money(&money, &stocked, &replaced), money_guide(engine, &stocked));
    });
}

/// A level under the view's (same band): activities gated above it
/// leave, accessibility is read at it; the catalogue view keeps them.
#[test]
fn a_lower_level_of_the_band_leaves_out_what_it_cannot_open() {
    let world = yields(false);
    let reads = Cell::new(0);
    with_engine(&world, &reads, |engine| {
        let view = money_view(engine, Some(30), &[], &[]);
        let (at_30, _) = finish_money(&view, &filter(Some(30)), &HashMap::new());
        let (at_27, _) = finish_money(&view, &filter(Some(27)), &HashMap::new());
        assert!(at_30.iter().any(|row| row.method == "smelt:steel_bar"), "steel opens at 30");
        assert!(!at_27.iter().any(|row| row.method == "smelt:steel_bar"), "not at 27");
        assert!(at_27.iter().any(|row| row.method == "gather:mining/iron_ore"), "iron opens at 15");
        assert!(!at_27.iter().any(|row| row.method == "gather:mining/coal"), "coal at 30");
        assert!(at_27.iter().all(|row| row.prereqs.levels.iter().all(|(_, needed)| *needed <= 27) || !crate::guide_feasible::with_filter_bot(&filter(Some(27)), |bot| crate::guide_feasible::feasible_for(row, bot).within_reach())));
    });
}

/// A chain whose upstream needs its own product (mining iron spends an
/// iron bar) ends: the cycle is cut at the object already downstream,
/// the autonomous smelting carries the source gap (D22), and the guides
/// build.
#[test]
fn a_cyclic_chain_ends_and_blocks_its_line() {
    let world = yields(true);
    let reads = Cell::new(0);
    with_engine(&world, &reads, |engine| {
        let unit = crate::guide_chain::chain_unit(engine, "smelt:iron_bar", "", "iron_bar", 30, 0, &[], crate::guide_chain::Sourcing::Farm);
        let unit = unit.expect("the smelt itself reads");
        assert!(!unit.gaps.is_empty(), "iron ore has no autonomous source without a bar: {unit:?}");
        let (training, _) = training_guide(engine, &filter(Some(30)));
        let smelt = training.iter().find(|row| row.method == "smelt:iron_bar" && row.supply == crate::guide_model::Supply::Autonomous).expect("autonomous smelt row");
        assert!(!smelt.priced && !smelt.prereqs.sources.is_empty(), "{smelt:?}");
        let (money, _) = money_guide(engine, &filter(Some(30)));
        assert!(money.iter().any(|row| row.method == "smelt:iron_bar"));
    });
}

/// Each yield is read once per build: a second guide of the same build
/// reads none, a set of views shares them.
#[test]
fn a_yield_is_read_once_per_build() {
    let world = yields(false);
    let reads = Cell::new(0);
    with_engine(&world, &reads, |engine| {
        let _first = money_guide(engine, &filter(Some(30)));
        let _training = training_guide(engine, &filter(Some(30)));
        let after = reads.get();
        assert!(after > 0);
        let _again = money_guide(engine, &filter(Some(30)));
        let _training_again = training_guide(engine, &filter(Some(30)));
        assert_eq!(reads.get(), after, "no yield read twice in one build");
    });
}

/// Time budget of the guides on the WHOLE F2P catalogue (regression of
/// 2026-11-17: minutes per request). The world: every method's game
/// prior as its yield, every object traded at its cost with deep asks,
/// the real F2P shops (finite stock, unit by unit) and shop sales. One
/// build of money and training at level 30 and their catalogue views
/// (each method at its gate: a D17 pass per gate level).
#[test]
fn the_whole_catalogue_builds_within_its_budget() {
    let rate = |ask: &RateAsk| {
        let query = crate::method_rate::PriorQuery { method: ask.method.clone(), level: ask.level, tool: ask.tool.clone(), site: ask.site };
        let prior = crate::method_rate::guide_prior(&query).map_err(|error| error.to_string())?;
        Ok(RateAnswer {
            flows: prior.flows.iter().map(|(item, cause, qty)| (item.clone(), cause.to_string(), *qty)).collect(),
            units: prior.units.iter().map(|(kind, subject, qty)| (kind.to_string(), subject.clone(), *qty)).collect(),
            pause_per_6000: 0.0,
            confidence: crate::guide_money::unmeasured(),
        })
    };
    let price = |item: &str| {
        let cost = lifescape_gamedata::objs_by_name(item).first().map(|obj| obj.price.max(1) as f64).unwrap_or(1.0);
        traded(cost)
    };
    let npc = |item: &str| lifescape_gamedata::shop_sale::npc_sale_value(item).map(|value| (value as f64, ShopNeed::default()));
    let shop = |item: &str| lifescape_gamedata::shop_buy::purchase_steps(item, 6000).into_iter().map(|step| (step.price as f64, step.units as f64, ShopNeed::default())).collect::<Vec<_>>();
    let quest = |_: &str| None;
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc, shop_buy: &shop, market_wait_ticks: Some(100.0), quest_ticks: &quest, measured: &[] };
    let began = std::time::Instant::now();
    let engine = Engine::new(&world);
    let (money, _) = money_guide(&engine, &filter(Some(30)));
    let (training, _) = training_guide(&engine, &filter(Some(30)));
    let at_30 = began.elapsed();
    let (catalogue, _) = money_guide(&engine, &filter(None));
    let (training_catalogue, _) = training_guide(&engine, &filter(None));
    let spent = began.elapsed();
    println!("guides budget: level 30 {at_30:?}, with catalogues {spent:?}");
    assert!(money.len() > 300 && training.len() > 300 && catalogue.len() > 300 && training_catalogue.len() > 300, "{} {} {} {}", money.len(), training.len(), catalogue.len(), training_catalogue.len());
    // Measured 2026-11-17 (release, cold process, loaded host): 2.4 s at
    // level 30, 5.4 s with both catalogues; before the fixes the
    // level-30 pair alone took over 25 s (a shop sale walked every F2P
    // shop at every price) and each catalogue minutes.
    assert!(at_30.as_secs_f64() < 10.0, "money + training at level 30 took {at_30:?}");
    assert!(spent.as_secs_f64() < 40.0, "with both catalogues {spent:?}");
}
