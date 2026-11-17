//! The market-wide evaluation (2026-11-14): real trips instead of a fixed
//! round trip, sales at the current price, public claims, alchemy floor,
//! training as an investment.

use super::{opportunity, preempts, ranked_opportunities, unit_cost, Producer};
use crate::market_fit::BookView;

fn yields(method: &str, _item: &str) -> f64 {
    if method.starts_with("smith:") || method.starts_with("smelt:") {
        600.0
    } else {
        200.0
    }
}

/// bst29bh9 live 2026-11-14: smithing 22, mining 55, iron ore in the
/// bank, a hammer.
fn smith_22(skill: &str) -> u8 {
    match skill {
        "smithing" => 22,
        "mining" => 55,
        "attack" | "strength" | "defence" => 20,
        _ => 1,
    }
}

fn ore_and_hammer(item: &str) -> i64 {
    match item {
        "hammer" => 1,
        "iron_ore" => 60,
        _ => 0,
    }
}

fn bot<'a>(level_of: &'a dyn Fn(&str) -> u8, held: &'a dyn Fn(&str) -> i64, coin_rate: f64) -> Producer<'a> {
    Producer {
        level_of,
        held,
        avoid: &[],
        quests_done: &[],
        coin_rate,
        units_per_6000: &yields,
        ironman: false,
        seed: 3,
        tick: 0,
        combat: 30,
        position: Some((3185, 3436, 0)),
        free_pass: false,
        planned_level: &super::no_plan,
        alch_net: &super::no_alch,
        rate: &crate::work_rate::prior_rate,
    }
}

fn helm_bids() -> BookView {
    BookView { bids: vec![(123, 3)], ..BookView::default() }
}

/// Live 2026-11-14: 3 iron full helm bids at 123, a smith at 22 with iron
/// ore in the bank. The fixed 800-tick round trip made it a loss (−559);
/// the real trip (Varrock west bank to a furnace, the anvil, the
/// fountain) is a few hundred ticks and the lot answers at a modest rate.
#[test]
fn a_smith_with_ore_answers_the_iron_helm_bids() {
    let none = |_: &str| None;
    let smith = bot(&smith_22, &ore_and_hammer, 0.1);
    let found = opportunity("iron_full_helm", &helm_bids(), &none, &smith).expect("makeable from its ore");
    assert_eq!(found.unit.plan.first().map(String::as_str), Some("make smith:iron_full_helm"), "{found:?}");
    assert!(found.unit.plan.iter().any(|step| step == "make smelt:iron_bar"), "smelts its own bars: {found:?}");
    assert!(found.unit.plan.iter().any(|step| step == "held iron_ore"), "{found:?}");
    assert_eq!(found.count, 2, "half the 3 bids, rounded up: {found:?}");
    assert!(found.ticks < 800.0 + found.unit.ticks * 2.0, "the real trip, not 800 per answer: {found:?}");
    assert!(found.unit.fixed_ticks > 0.0, "walks between furnace, anvil and fountain: {found:?}");
    assert!(preempts(&found, smith.coin_rate), "{found:?}");
    // A bot whose hour is worth more keeps its ambition.
    let busy = bot(&smith_22, &ore_and_hammer, 2.0);
    let found = opportunity("iron_full_helm", &helm_bids(), &none, &busy);
    assert!(found.as_ref().is_none_or(|found| !preempts(found, busy.coin_rate)), "{found:?}");
}

/// No bid at all, but the object trades: the market takes a share of a
/// day's volume at the current price, behind the asks already resting
/// there; public claims of other producers come off.
#[test]
fn a_traded_object_without_bids_sells_at_its_current_price() {
    let none = |_: &str| None;
    let smith = bot(&smith_22, &ore_and_hammer, 0.1);
    let traded = BookView { asks: vec![(140, 10), (400, 50)], volume_24h: 90, current: Some(150), ..BookView::default() };
    let found = opportunity("iron_full_helm", &traded, &none, &smith).expect("sales at 150");
    // 45 of the day's 90 at 150, 10 already rest at or under it: 35, half.
    assert_eq!((found.count, found.from_sales, found.from_bids), (18, 18, 0), "{found:?}");
    assert!((found.revenue - 18.0 * 150.0).abs() < 1e-6, "{found:?}");
    let claimed = BookView { claimed: 30, ..traded.clone() };
    let found = opportunity("iron_full_helm", &claimed, &none, &smith).expect("5 left");
    assert_eq!(found.count, 3, "35 − 30 claimed = 5, half: {found:?}");
    let never_traded = BookView { asks: vec![(400, 50)], volume_24h: 0, current: None, ..BookView::default() };
    assert_eq!(opportunity("iron_full_helm", &never_traded, &none, &smith), None, "no bid, no price: no value");
}

/// The bot's own alchemy floor values a lot nobody bids for: one
/// inventory, at the net of the cast.
#[test]
fn a_caster_values_its_alchemy_floor() {
    let none = |_: &str| None;
    let alch = |item: &str| (item == "iron_full_helm").then_some(80.0);
    let mut caster = bot(&smith_22, &ore_and_hammer, 0.1);
    caster.alch_net = &alch;
    let empty = BookView::default();
    let found = opportunity("iron_full_helm", &empty, &none, &caster).expect("alched");
    assert_eq!((found.count, found.from_alch), (crate::market_value::ALCH_LOT, crate::market_value::ALCH_LOT), "{found:?}");
    let plain = bot(&smith_22, &ore_and_hammer, 0.1);
    assert_eq!(opportunity("iron_full_helm", &empty, &none, &plain), None);
}

/// The whole market is weighed, not only bids: ranked by rate, an object
/// the bot cannot make never ranks, Iron never answers.
#[test]
fn every_valued_object_is_weighed() {
    let none = |_: &str| None;
    let smith = bot(&smith_22, &ore_and_hammer, 0.1);
    let books = vec![
        ("iron_full_helm".to_string(), helm_bids()),
        ("iron_dagger".to_string(), BookView { volume_24h: 200, current: Some(60), ..BookView::default() }),
        ("rune_platebody".to_string(), BookView { bids: vec![(60_000, 2)], ..BookView::default() }),
    ];
    let mut all = smith;
    // Every window answers in this test.
    let ranked: Vec<String> = (0..64u64)
        .find_map(|seed| {
            all.seed = seed;
            let found = ranked_opportunities(&books, &none, &all);
            (found.len() >= 2).then(|| found.into_iter().map(|found| found.item).collect())
        })
        .expect("a seed answers both");
    assert!(ranked.contains(&"iron_full_helm".to_string()) && ranked.contains(&"iron_dagger".to_string()), "{ranked:?}");
    assert!(!ranked.contains(&"rune_platebody".to_string()), "smithing 99 is out of reach: {ranked:?}");
}

/// Training is an investment: a bot whose ambition climbs smithing anyway
/// carries none of it; the products made while training pay part of it.
#[test]
fn training_is_amortized_not_charged_to_one_lot() {
    let ores = |item: &str| match item {
        "copper_ore" | "tin_ore" => Some(BookView { asks: vec![(3, 5000)], ..BookView::default() }),
        "iron_ore" => Some(BookView { asks: vec![(5, 5000)], ..BookView::default() }),
        _ => None,
    };
    let fresh = |skill: &str| if skill == "smithing" { 10 } else { 30 };
    let none_held = |item: &str| i64::from(item == "hammer");
    let lone = bot(&fresh, &none_held, 0.1);
    let unit = super::unit_cost_training("iron_full_helm", 3, &ores, &lone).expect("trains 10 → 22");
    assert_eq!(unit.train, vec![("smithing".to_string(), 10, 22)], "{unit:?}");
    let (coins, ticks) = super::training_cost(&unit.train, &ores, &lone).expect("gross");
    let (charged_coins, charged_ticks) = super::training_charge(&unit.train, &ores, &lone).expect("charged");
    assert!(charged_ticks <= ticks / crate::market_value::TRAINING_AMORTIZE_LOTS + 1e-6, "{charged_ticks} of {ticks}");
    assert!(charged_coins <= coins / crate::market_value::TRAINING_AMORTIZE_LOTS + 1e-6);
    let planned = |skill: &str| if skill == "smithing" { 40 } else { 0 };
    let mut smith = bot(&fresh, &none_held, 0.1);
    smith.planned_level = &planned;
    assert_eq!(super::training_charge(&unit.train, &ores, &smith), Some((0.0, 0.0)), "its ambition climbs smithing anyway");
    // Products of the training that sell pay part of it.
    let with_market = |item: &str| match item {
        "bronze_dagger" | "bronze_med_helm" | "bronze_scimitar" | "bronze_sword" | "bronze_platebody" | "bronze_2h_sword" | "bronze_warhammer" => {
            Some(BookView { volume_24h: 100_000, current: Some(500), ..BookView::default() })
        }
        _ => ores(item),
    };
    let paid = super::training_charge(&unit.train, &with_market, &lone).expect("charged");
    assert!(paid.0 + paid.1 * 0.1 < charged_coins + charged_ticks * 0.1, "{paid:?}");
    assert!(unit_cost("iron_full_helm", 3, &ores, &lone).is_none(), "no training in a plain unit cost");
}
