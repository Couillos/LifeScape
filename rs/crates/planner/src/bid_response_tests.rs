//! Producers respond to bids: unit cost (buy vs make, time included),
//! feasibility, depth split, the preempt rule and the fleet spread.

use super::{answers_now, best_opportunity, bid_goal, opportunity, preempts, unit_cost, Producer, BID_PREFIX, PRODUCER_SPLIT};
use crate::market_fit::BookView;
use crate::model::GoalKind;

/// Live 2026-11-14: 18 bronze scimitar bids from 1774 up to 2961, bronze
/// bars resting at 5 (146 units), nobody producing.
fn scimitar_bids() -> BookView {
    let prices = [2961, 2896, 2876, 2484, 2434, 2133, 1967, 1930, 1774, 1500, 1400, 1300, 1200, 1100, 1000, 900, 800, 700];
    BookView { bids: prices.iter().map(|price| (*price, 1)).collect(), asks: Vec::new(), volume_24h: 0, ..BookView::default() }
}

fn bars_at_five() -> BookView {
    BookView { bids: Vec::new(), asks: vec![(5, 146)], volume_24h: 40, ..BookView::default() }
}

fn market(item: &str) -> Option<BookView> {
    match item {
        "bronze_scimitar" => Some(scimitar_bids()),
        "bronze_bar" => Some(bars_at_five()),
        "hammer" => Some(BookView { bids: Vec::new(), asks: vec![(12, 3)], volume_24h: 0, ..BookView::default() }),
        _ => None,
    }
}

fn smith_levels(skill: &str) -> u8 {
    match skill {
        "smithing" => 20,
        "mining" => 20,
        _ => 1,
    }
}

fn hammer_only(item: &str) -> i64 {
    i64::from(item == "hammer")
}

fn yields(method: &str, _item: &str) -> f64 {
    if method.starts_with("smith:") { 600.0 } else { 200.0 }
}

fn smith<'a>(level_of: &'a dyn Fn(&str) -> u8, held: &'a dyn Fn(&str) -> i64, avoid: &'a [String], seed: u64) -> Producer<'a> {
    Producer {
        level_of,
        held,
        avoid,
        quests_done: &[],
        coin_rate: 0.5,
        units_per_6000: &yields,
        ironman: false,
        seed,
        tick: 0,
        combat: 3,
        position: None,
        free_pass: false,
        planned_level: &super::no_plan,
        alch_net: &super::no_alch,
        rate: &crate::work_rate::prior_rate,
    }
}

/// Seed whose word answers `item` in window 0.
fn answering_seed(item: &str) -> u64 {
    (0..64).find(|seed| answers_now(*seed, item, 0)).expect("one seed in 64 answers")
}

/// Bars at the public ask beat smelting them (time at the coin rate),
/// the hammer is held, smithing 20 meets the scimitar's gate.
#[test]
fn a_unit_costs_the_cheapest_inputs_plus_the_bots_time() {
    let avoid: Vec<String> = Vec::new();
    let bot = smith(&smith_levels, &hammer_only, &avoid, 1);
    let unit = unit_cost("bronze_scimitar", 9, &market, &bot).expect("makeable");
    assert_eq!(unit.plan.first().map(String::as_str), Some("make smith:bronze_scimitar"), "{unit:?}");
    assert!(unit.plan.iter().any(|step| step == "buy bronze_bar"), "bars bought at 5: {unit:?}");
    assert!((unit.coins - 10.0).abs() < 1e-6, "two bars at 5: {unit:?}");
    assert!((unit.ticks - 10.0).abs() < 1e-6, "600 per 6000 ticks: {unit:?}");
    // No bars on the market: the smith (mining 20) mines the ores itself
    // (`farm:<ore>` is the mining leg, its pickaxe priced as a tool) and
    // smelts them.
    let no_bars = |item: &str| if item == "bronze_bar" { None } else { market(item) };
    let mined = unit_cost("bronze_scimitar", 9, &no_bars, &bot).expect("mined and smelted");
    assert_eq!(mined.plan, ["make smith:bronze_scimitar", "make smelt:bronze_bar", "make farm:copper_ore", "make farm:tin_ore"], "{mined:?}");
    assert!(mined.ticks > unit.ticks, "mining and smelting take the bot's time: {mined:?}");
}

/// Level, ban and tool gates: a level-1 smith, a banned smith and a smith
/// without hammer nor any seller of one cannot answer.
#[test]
fn feasibility_reads_levels_bans_and_tools() {
    let fresh = |_: &str| 1u8;
    let none = |_: &str| 0i64;
    let avoid: Vec<String> = Vec::new();
    let banned = vec!["smithing".to_string()];
    assert_eq!(unit_cost("bronze_scimitar", 1, &market, &smith(&fresh, &hammer_only, &avoid, 1)), None, "smithing 1 < 5");
    assert_eq!(unit_cost("bronze_scimitar", 1, &market, &smith(&smith_levels, &hammer_only, &banned, 1)), None, "banned");
    let no_hammer_seller = |item: &str| if item == "hammer" { None } else { market(item) };
    let bot = smith(&smith_levels, &none, &avoid, 1);
    let with_shop = unit_cost("bronze_scimitar", 1, &no_hammer_seller, &bot);
    assert!(with_shop.is_some(), "a hammer is an NPC shop object (D-A): {with_shop:?}");
    let bought = unit_cost("bronze_scimitar", 4, &market, &bot).expect("hammer at the ask");
    assert!((bought.coins - 10.0).abs() < 1e-6, "bars per unit: {bought:?}");
    assert!((bought.fixed_coins - 12.0).abs() < 1e-6, "the hammer is bought once for the lot: {bought:?}");
    let spread = bought.total_for(4, 0.0) - bought.total_for(1, 0.0);
    assert!((spread + 12.0 * 3.0 / 4.0).abs() < 1e-6, "and spreads over the units: {spread}");
}

/// The live book: a smith fills half of what pays (split), never the
/// unprofitable tail, and the opportunity preempts its ambition.
#[test]
fn the_scimitar_bids_draw_a_production_quota() {
    let avoid: Vec<String> = Vec::new();
    let seed = answering_seed("bronze_scimitar");
    let bot = smith(&smith_levels, &hammer_only, &avoid, seed);
    let found = opportunity("bronze_scimitar", &scimitar_bids(), &market, &bot).expect("pays");
    assert_eq!(found.count, (18 + PRODUCER_SPLIT - 1) / PRODUCER_SPLIT, "{found:?}");
    assert_eq!(found.best_bid, 2961);
    assert!(found.revenue >= 9.0 * 1774.0, "the 9 best bids: {found:?}");
    assert!(preempts(&found, bot.coin_rate), "{found:?}");
    let books = vec![("bronze_scimitar".to_string(), scimitar_bids())];
    let best = best_opportunity(&books, &market, &bot).expect("answered");
    let (goal, origin) = bid_goal(&best);
    assert_eq!(goal, GoalKind::Produce { item: "bronze_scimitar".to_string(), count: 9 });
    assert_eq!(origin, format!("{BID_PREFIX}bronze_scimitar"));
}

/// Supply already on its way (asks just above the bids, the bot's own
/// stock) comes off the depth; a bid under the unit cost never pays; a
/// bot whose time is worth more than the bid rate keeps its ambition.
#[test]
fn pending_supply_held_stock_and_rich_time_hold_the_bot_back() {
    let avoid: Vec<String> = Vec::new();
    let seed = answering_seed("bronze_scimitar");
    let bot = smith(&smith_levels, &hammer_only, &avoid, seed);
    let mut crowded = scimitar_bids();
    crowded.asks.push((3200, 16));
    let found = opportunity("bronze_scimitar", &crowded, &market, &bot).expect("2 left");
    assert_eq!(found.count, 1, "18 bids − 16 pending = 2, half each: {found:?}");
    let stocked = |item: &str| if item == "bronze_scimitar" { 18 } else { hammer_only(item) };
    assert_eq!(opportunity("bronze_scimitar", &scimitar_bids(), &market, &smith(&smith_levels, &stocked, &avoid, seed)), None, "it sells what it holds");
    let cheap = BookView { bids: vec![(9, 5)], asks: Vec::new(), volume_24h: 0, ..BookView::default() };
    assert_eq!(opportunity("bronze_scimitar", &cheap, &market, &bot), None, "9 < 10 coins of bars + time");
    let mut rich = smith(&smith_levels, &hammer_only, &avoid, seed);
    rich.coin_rate = 200.0;
    let found = opportunity("bronze_scimitar", &scimitar_bids(), &market, &rich);
    assert!(found.as_ref().is_none_or(|found| !preempts(found, rich.coin_rate)), "{found:?}");
}

/// Buying the object itself to resell is the merchants' trade: a bid the
/// bot could only fill from the asks is no production.
#[test]
fn a_producer_never_flips_the_product() {
    let banned = vec!["smithing".to_string()];
    let mut book = scimitar_bids();
    book.asks.push((10_000, 5));
    let flip_only = |item: &str| if item == "bronze_scimitar" { Some(book.clone()) } else { market(item) };
    assert_eq!(opportunity("bronze_scimitar", &book, &flip_only, &smith(&smith_levels, &hammer_only, &banned, 1)), None);
}

/// Live 2026-11-14: the whole fleet had smithing 1 and nobody answered
/// 18 bronze scimitar bids up to 2961. A level-1 bot trains to 5 (bronze
/// bars smelted from ores on sale) for a bid worth it: the training is a
/// one-off cost of the lot, and the quota still preempts. A gap past
/// `MAX_TRAIN_LEVELS` never answers; a plain unit cost never trains.
#[test]
fn a_level_one_bot_trains_for_a_bid_worth_it() {
    let avoid: Vec<String> = Vec::new();
    let fresh = |_: &str| 1u8;
    let with_ores = |item: &str| match item {
        "copper_ore" | "tin_ore" => Some(BookView { bids: Vec::new(), asks: vec![(3, 500)], volume_24h: 0, ..BookView::default() }),
        _ => market(item),
    };
    let seed = answering_seed("bronze_scimitar");
    let bot = smith(&fresh, &hammer_only, &avoid, seed);
    assert_eq!(unit_cost("bronze_scimitar", 1, &with_ores, &bot), None, "no training in a plain unit cost");
    let found = opportunity("bronze_scimitar", &scimitar_bids(), &with_ores, &bot).expect("worth training");
    assert_eq!(found.unit.train, vec![("smithing".to_string(), 1, 5)], "{found:?}");
    let (coins, ticks) = super::training_cost(&found.unit.train, &with_ores, &bot).expect("priced");
    assert!(coins > 0.0 && ticks > 0.0, "ores bought, smelting time: {coins} {ticks}");
    // An investment: the lot carries one share of it (no plan climbs
    // smithing, no market for the training products here).
    let (charged_coins, charged_ticks) = super::training_charge(&found.unit.train, &with_ores, &bot).expect("charged");
    let share = 1.0 / crate::market_value::TRAINING_AMORTIZE_LOTS;
    assert!((charged_coins - coins * share).abs() < 1e-6 && (charged_ticks - ticks * share).abs() < 1e-6, "{charged_coins} {charged_ticks}");
    assert!(found.ticks >= charged_ticks + crate::market_value::HANDOVER_TICKS, "{found:?}");
    assert!(preempts(&found, bot.coin_rate), "{found:?}");
    let iron_bids = BookView { bids: vec![(5000, 4)], asks: Vec::new(), volume_24h: 0, ..BookView::default() };
    let iron_market = |item: &str| match item {
        "iron_bar" => Some(BookView { bids: Vec::new(), asks: vec![(20, 100)], volume_24h: 0, ..BookView::default() }),
        _ => with_ores(item),
    };
    assert_eq!(opportunity("iron_platebody", &iron_bids, &iron_market, &bot), None, "smithing 33 is 32 levels away");
}

/// The spread: half the windows answer, the other half wait; different
/// bots answer different windows; Iron never answers.
#[test]
fn bots_spread_over_windows_and_iron_never_answers() {
    let answered = (0..200u64).filter(|window| answers_now(7, "bronze_scimitar", window * super::BID_WINDOW_TICKS)).count();
    assert!((60..140).contains(&answered), "about one window in two: {answered}");
    let bots_now = (0..200u64).filter(|seed| answers_now(*seed, "bronze_scimitar", 0)).count();
    assert!((60..140).contains(&bots_now), "about one bot in two per window: {bots_now}");
    let avoid: Vec<String> = Vec::new();
    let mut iron = smith(&smith_levels, &hammer_only, &avoid, answering_seed("bronze_scimitar"));
    iron.ironman = true;
    assert_eq!(best_opportunity(&[("bronze_scimitar".to_string(), scimitar_bids())], &market, &iron), None);
}

/// R7 suite (2026-11-17): the training a bid carries reads the MEASURED
/// yields (`Producer::rate`, the planner's reader: n >= 3 blended with the
/// prior, the prior below), never the prior alone: a smelt measured to
/// burn twice the ore per XP doubles the ores the training buys.
#[test]
fn a_bids_training_reads_the_measured_yields() {
    let avoid: Vec<String> = Vec::new();
    let fresh = |_: &str| 1u8;
    let with_ores = |item: &str| match item {
        "copper_ore" | "tin_ore" => Some(BookView { bids: Vec::new(), asks: vec![(3, 5000)], volume_24h: 0, ..BookView::default() }),
        _ => market(item),
    };
    let seed = answering_seed("bronze_scimitar");
    let prior_bot = smith(&fresh, &hammer_only, &avoid, seed);
    let train = vec![("smithing".to_string(), 1u8, 5u8)];
    let (prior_coins, prior_ticks) = super::training_cost(&train, &with_ores, &prior_bot).expect("priced on the prior");
    let measured = |method: &str, level: u8| {
        crate::work_rate::prior_rate(method, level).map(|mut rate| {
            for flow in rate.flows_6000.iter_mut().filter(|flow| flow.1 == "input") {
                flow.2 *= 2.0;
            }
            rate
        })
    };
    let measured_bot = Producer { rate: &measured, ..smith(&fresh, &hammer_only, &avoid, seed) };
    let (coins, ticks) = super::training_cost(&train, &with_ores, &measured_bot).expect("priced on the measure");
    assert!(coins > prior_coins * 1.5, "measured {coins} vs prior {prior_coins}");
    assert!((ticks - prior_ticks).abs() < 1e-6, "the ores are bought: the smelting time is the game's, {ticks} vs {prior_ticks}");
}
