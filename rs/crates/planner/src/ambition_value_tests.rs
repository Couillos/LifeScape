//! A4 (user 2026-11-21): the ambition that balances the market, measured
//! by the gain per 6000 ticks of serving each unserved demand, the climb
//! included (A1).

use super::*;
use crate::investment::{best_xp_rate, investment, investment_ladder, AlchOffer, INVESTMENT_HORIZON_TICKS};
use crate::work_rate::prior_rate;

fn bot(account: &str, levels: &[(&str, u8)]) -> FleetBot {
    FleetBot {
        account: account.to_string(),
        levels: levels.iter().map(|(skill, level)| (skill.to_string(), *level)).collect(),
        ironman: false,
        market_affinity: 0.5,
        seed: 7,
        kind: None,
        market_maker: false,
        operator: false,
    }
}

fn holding(account: &str, kind: &str, levels: &[(&str, u8)]) -> FleetBot {
    FleetBot { kind: Some(kind.to_string()), ..bot(account, levels) }
}

fn bid(item: &str, unit_price: f64, unmet: i64, asked: bool) -> UnservedBid {
    UnservedBid { item: item.to_string(), unit_price, unmet, asked, ladder: Vec::new() }
}

/// Market prices of the fixtures (coins per unit); the strung diamond
/// amulet and the wool stay unpriced: whoever makes the amulet makes them.
fn price(name: &str) -> Option<f64> {
    match name {
        "cosmicrune" => Some(150.0),
        "earthrune" => Some(5.0),
        "naturerune" => Some(200.0),
        "firerune" => Some(5.0),
        "coal" => Some(150.0),
        "logs" => Some(4.0),
        _ => None,
    }
}

fn xp_rate(skill: &str, level: u8) -> Option<f64> {
    best_xp_rate(skill, level)
}

fn rate(method: &str, level: u8) -> Option<crate::work_rate::WorkRate> {
    prior_rate(method, level)
}

fn context<'a>(bids: &'a [UnservedBid], offers: &'a [AlchOffer]) -> ValueContext<'a> {
    ValueContext { bids, alch_offers: offers, surplus: &[], price: &price, xp_rate: &xp_rate, rate: &rate, roster: None, targets: &[] }
}

fn score(found: &Option<Pick>) -> f64 {
    found.as_ref().map(|pick| pick.score).unwrap_or(0.0)
}

/// A crafting-60 bot, an amulet of power wanted at 1 M by 66 buyers and
/// coal wanted by miners' buyers: the crafter ranks ahead of the miner
/// (the amulet's crafting steps are ten levels away, its value is a
/// million a unit; the coal is the ore's worth).
#[test]
fn a_crafting_bot_with_an_amulet_bid_gets_the_crafter_before_the_miner() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false), bid("coal", 150.0, 2000, false)];
    let ctx = context(&bids, &[]);
    let crafter = bot("c0", &[("crafting", 60)]);
    let by_kind = |kind: &str| kind_value(&crafter, kind, &ctx);
    let crafting = by_kind("crafter");
    let mining = by_kind("miner");
    assert!(crafting.value_6000 > mining.value_6000, "crafter {crafting:?} against miner {mining:?}");
    assert_eq!(crafting.driver.as_ref().map(|(item, _)| item.as_str()), Some("amulet_of_power"));
    let climb = crafting.driver.as_ref().map(|(_, ticks)| *ticks).unwrap_or(0.0);
    assert!(climb > 0.0, "crafting 60 -> 70 is a real climb: {climb}");
    let other = ctx_pick(&crafter, &[], &ctx);
    assert!(score(&other) > 0.0, "{other:?}");
}

fn ctx_pick(who: &FleetBot, held: &[&str], ctx: &ValueContext) -> Option<Pick> {
    pick_by_value(who, held, ctx, &[])
}

/// A long climb that unlocks a lucrative production beats a fast line
/// nobody buys: the amulet (magic 57 from magic 1, a long climb) against
/// logs at 4 coins wanted at once (no level to climb).
#[test]
fn a_long_lucrative_climb_beats_a_fast_line_without_outlet() {
    let fresh = bot("m0", &[]);
    let level_of = |skill: &str| fresh.level(skill);
    let amulet = investment("amulet_of_power", 1_000_000.0, 66.0, &["magic"], &level_of, &price, &xp_rate, &rate).expect("an amulet investment");
    let logs = investment("logs", 4.0, 1000.0, &["woodcutting"], &level_of, &price, &xp_rate, &rate).expect("a logs investment");
    assert!(amulet.climb_ticks > 10_000.0 && logs.climb_ticks == 0.0, "amulet {:.0} ticks, logs {:.0}", amulet.climb_ticks, logs.climb_ticks);
    assert!(amulet.value_6000 > 100.0 * logs.value_6000, "amulet {amulet:?} against logs {logs:?}");
    // The same through the kinds.
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false), bid("logs", 4.0, 1000, false)];
    let ctx = context(&bids, &[]);
    let pick = pick_by_value(&fresh, &[], &ctx, &[]).expect("a pick");
    assert_ne!(pick.kind, "woodcutter", "{pick:?}");
}

/// A climb longer than the horizon pays nothing; a longer climb pays less
/// than a shorter one (the amortization).
#[test]
fn the_horizon_bounds_a_climb() {
    let level_of_low = |_: &str| 1u8;
    let level_of_near = |skill: &str| if skill == "magic" { 50 } else { 1 };
    let far = investment("amulet_of_power", 1_000_000.0, 1_000_000.0, &["magic"], &level_of_low, &price, &xp_rate, &rate).expect("far");
    let near = investment("amulet_of_power", 1_000_000.0, 1_000_000.0, &["magic"], &level_of_near, &price, &xp_rate, &rate).expect("near");
    assert!(near.climb_ticks < far.climb_ticks && near.value_6000 > far.value_6000, "near {near:?} far {far:?}");
    let never = |_: &str, _: u8| Some(1e-6);
    let hopeless = investment("amulet_of_power", 1_000_000.0, 1e9, &["magic"], &level_of_low, &price, &never, &rate).expect("hopeless");
    assert!(hopeless.climb_ticks > INVESTMENT_HORIZON_TICKS && hopeless.value_6000 == 0.0, "{hopeless:?}");
}

/// The capacity bounds the value: one unit wanted at 1 M weighs less than
/// the same price wanted sixty-six times.
#[test]
fn one_unit_wanted_weighs_less_than_a_standing_demand() {
    let level_of = |_: &str| 99u8;
    let one = investment("amulet_of_power", 1_000_000.0, 1.0, &["magic"], &level_of, &price, &xp_rate, &rate).expect("one");
    let many = investment("amulet_of_power", 1_000_000.0, 66.0, &["magic"], &level_of, &price, &xp_rate, &rate).expect("many");
    assert!(many.value_6000 > 10.0 * one.value_6000, "one {one:?} many {many:?}");
}

/// Alchemy: the magic level that opens it, valued on the offers that pay
/// after the runes.
#[test]
fn magic_55_is_valued_by_its_alchemy() {
    let offers = vec![AlchOffer { item: "rune_platebody".to_string(), price: 20_000.0, units: 40.0 }];
    let mage_low = bot("m1", &[("magic", 40)]);
    let none = kind_value(&mage_low, "mage", &context(&[], &[]));
    let with = kind_value(&mage_low, "mage", &context(&[], &offers));
    assert!(with.value_6000 > none.value_6000, "{with:?} against {none:?}");
    assert_eq!(with.driver.as_ref().map(|(item, _)| item.as_str()), Some("alchemy"), "{with:?}");
}

/// The bots already on a kind share its value (the user's `/ (holders +
/// 1)`): with one mage there, the next bot's mage score is half.
#[test]
fn the_bots_already_on_a_kind_share_its_value() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false)];
    let ctx = context(&bids, &[]);
    let fresh = bot("c1", &[("magic", 60)]);
    let alone = kind_value(&fresh, "mage", &ctx);
    let alone_pick = pick_by_value(&fresh, &[], &ctx, &["miner", "runecrafter", "crafter", "smith"]).expect("alone");
    let shared = pick_by_value(&fresh, &["mage"], &ctx, &["miner", "runecrafter", "crafter", "smith"]).expect("shared");
    assert_eq!(alone_pick.kind, "mage");
    assert!((shared.score - alone.value_6000 / 2.0).abs() < 1e-6 * alone.value_6000.max(1.0), "alone {alone:?} shared {shared:?}");
    assert_eq!(shared.holders, 1);
}

/// The bots assigned DURING the pass cover the demand they are given: a
/// mage makes far more than the 66 amulets, so the second mage of the
/// pass is worth nothing there and goes to the next demand (the coal),
/// never piling on the same bid.
#[test]
fn a_demand_covered_in_the_pass_is_no_longer_unserved() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false), bid("coal", 150.0, 2000, false)];
    let ctx = context(&bids, &[]);
    let bots = vec![bot("m1", &[("magic", 60)]), bot("m2", &[("magic", 60)]), bot("m3", &[("magic", 60)])];
    let moved = rebalance(&bots, &ctx);
    let kinds: Vec<&str> = moved.iter().map(|row| row.pick.kind.as_str()).collect();
    assert_eq!(kinds.iter().filter(|kind| **kind == "mage").count(), 1, "one mage covers the amulet bid: {moved:?}");
    assert!(moved.iter().any(|row| row.pick.kind == "miner" || row.pick.kind == "crafter" || row.pick.kind == "runecrafter" || row.pick.kind == "smith"), "{moved:?}");
}

/// Deterministic: the same inputs, the same kind; no seed enters.
#[test]
fn the_pick_is_deterministic_and_seed_free() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false), bid("coal", 150.0, 2000, true)];
    let ctx = context(&bids, &[]);
    let mut left = bot("d0", &[("crafting", 60)]);
    let mut right = bot("d0", &[("crafting", 60)]);
    left.seed = 1;
    right.seed = 987_654_321;
    assert_eq!(pick_by_value(&left, &[], &ctx, &[]), pick_by_value(&right, &[], &ctx, &[]));
}

/// No demand anywhere: the fewest holders, then the best fit (a woodcutter
/// of 70 among levels of 1 takes woodcutting); never a random kind.
#[test]
fn with_no_demand_the_fewest_holders_then_the_best_fit() {
    let ctx = context(&[], &[]);
    let woodcutter = bot("w0", &[("woodcutting", 70)]);
    let pick = pick_by_value(&woodcutter, &[], &ctx, &[]).expect("a pick");
    assert_eq!(pick.kind, "woodcutter", "{pick:?}");
    let held = ["woodcutter"];
    let next = pick_by_value(&woodcutter, &held, &ctx, &[]).expect("a pick");
    assert_ne!(next.kind, "woodcutter", "a kind nobody holds comes first: {next:?}");
}

/// The rebalance never touches Zezima, N0valyfe, Yogosun or an operator,
/// counts their kinds as held, and reassigns everyone else.
#[test]
fn the_rebalance_leaves_the_protected_and_the_operators_alone() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false), bid("coal", 150.0, 2000, false)];
    let ctx = context(&bids, &[]);
    let mut pinned = holding("pinned_one", "woodcutter", &[]);
    pinned.operator = true;
    let mut maker = holding("maker", "market_maker", &[]);
    maker.market_maker = true;
    let bots = vec![
        holding("Zezima", "woodcutter", &[("woodcutting", 99)]),
        holding("n0valyfe", "fisher", &[]),
        holding("YOGOSUN", "cook", &[]),
        pinned,
        maker,
        holding("a1", "woodcutter", &[("crafting", 60)]),
        holding("a2", "woodcutter", &[("crafting", 61)]),
        holding("a3", "fisher", &[("mining", 50)]),
    ];
    let moved = rebalance(&bots, &ctx);
    let names: Vec<&str> = moved.iter().map(|row| row.account.as_str()).collect();
    assert_eq!(names, vec!["a1", "a2", "a3"], "only the movable bots: {moved:?}");
    for protected_name in ["Zezima", "n0valyfe", "YOGOSUN", "pinned_one", "maker"] {
        assert!(!names.contains(&protected_name));
    }
    assert!(moved.iter().all(|row| row.pick.value_6000 >= 0.0 && !row.pick.kind.is_empty()));
    // The balance: the crafters do not all pile on one kind.
    let kinds: std::collections::BTreeSet<&str> = moved.iter().map(|row| row.pick.kind.as_str()).collect();
    assert!(kinds.len() >= 2, "three bots, the demand shared: {moved:?}");
    assert!(protected("zezima") && protected("N0VALYFE") && protected("Yogosun") && !protected("a1"));
}

/// The numbers of a pick ride in its log line.
#[test]
fn a_pick_states_its_numbers() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false)];
    let ctx = context(&bids, &[]);
    let pick = pick_by_value(&bot("n0", &[("crafting", 60)]), &[], &ctx, &[]).expect("pick");
    assert!(pick.why().starts_with("value:"), "{}", pick.why());
    assert!(pick.numbers().contains("per 6000 ticks"), "{}", pick.numbers());
}

/// Only the climb separates two bots (A4 point 2): the same unserved bid,
/// capacity so large that the gain over the horizon binds, one mage a few
/// levels from the enchant (57), the other from level 1: the advanced bot's
/// value is higher, and a bot already past the gate is worth the most.
#[test]
fn only_the_climb_time_separates_an_advanced_bot_from_a_beginner() {
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 1_000_000_000, false)];
    let ctx = context(&bids, &[]);
    let value = |level: u8| kind_value(&bot("b", &[("magic", level)]), "mage", &ctx);
    let (beginner, advanced, done) = (value(1), value(50), value(60));
    assert!(beginner.value_6000 < advanced.value_6000 && advanced.value_6000 < done.value_6000, "{beginner:?} {advanced:?} {done:?}");
    let climb = |row: &KindValue| row.driver.as_ref().map(|(_, ticks)| *ticks).unwrap_or(0.0);
    assert!(climb(&beginner) > climb(&advanced) && climb(&advanced) > 0.0 && climb(&done) == 0.0);
}

/// The rebalance is not brutal: a bot keeps its kind unless the best one
/// beats it by `MIN_GAIN_TO_CHANGE`, both measured alike (logs for the
/// woodcutter against coal for the miner, 10 % then 50 % better).
#[test]
fn a_bot_changes_only_when_the_gain_beats_the_margin() {
    let holder = holding("w1", "woodcutter", &[("mining", 30)]);
    let run = |coal_units: i64| {
        let bids = vec![bid("logs", 4.0, 1500, false), bid("coal", 150.0, coal_units, false)];
        let ctx = context(&bids, &[]);
        rebalance(&[holder.clone()], &ctx).remove(0).pick
    };
    let slight = run(44);
    assert_eq!(slight.kind, "woodcutter", "+10 % is not enough: {slight:?}");
    assert!(slight.kept);
    let clear = run(60);
    assert_eq!(clear.kind, "miner", "+50 % changes it: {clear:?}");
    assert!(!clear.kept);
    assert!((MIN_GAIN_TO_CHANGE - 0.25).abs() < 1e-12);
}

/// Consumer trades destroy objects and piles are settled by consuming more
/// (user rule): the outlet of a pile (what one bot of the kind burns over
/// the horizon, bounded by the pile left, at the object's price) is part of
/// the SCORE. A pile of 1-coin feathers weighs little against dear logs; a
/// pile of dear logs gives a firemaker a positive value; the bots assigned
/// in the pass use the pile up.
#[test]
fn the_outlet_of_a_pile_counts_in_the_score() {
    let surplus = vec![("feather".to_string(), 1_000_000.0), ("logs".to_string(), 1_000_000.0)];
    let priced = |name: &str| match name {
        "feather" => Some(1.0),
        "logs" => Some(40.0),
        other => price(other),
    };
    let ctx = ValueContext { bids: &[], alch_offers: &[], surplus: &surplus, price: &priced, xp_rate: &xp_rate, rate: &rate, roster: None, targets: &[] };
    let value = |kind: &str| kind_value(&bot("f1", &[]), kind, &ctx);
    let (fisher, firemaker) = (value("fisher"), value("firemaker"));
    assert!(fisher.value_6000 > 0.0 && (fisher.value_6000 - fisher.outlet_6000).abs() < 1e-9, "{fisher:?}");
    assert!(firemaker.value_6000 > 10.0 * fisher.value_6000, "dear logs weigh more than 1-coin feathers: {firemaker:?} {fisher:?}");
    // The assigned bots use the pile up: a small pile is gone after one bot.
    let small = vec![("logs".to_string(), 50.0)];
    let small_ctx = ValueContext { bids: &[], alch_offers: &[], surplus: &small, price: &priced, xp_rate: &xp_rate, rate: &rate, roster: None, targets: &[] };
    let moved = rebalance(&[bot("a1", &[]), bot("a2", &[])], &small_ctx);
    let firemakers = moved.iter().filter(|row| row.pick.kind == "firemaker").count();
    assert_eq!(firemakers, 1, "50 logs feed one firemaker, the second finds the pile gone: {moved:?}");
}

/// A bot works one thing at a time: one assigned miner covers only its
/// share of each of the two ore lines it answers, so the second bot still
/// finds unserved demand on the ore and takes it too (were one miner to
/// cover both lines in full, the second would see none).
#[test]
fn an_assigned_bot_covers_its_share_of_each_line_it_answers() {
    let level_of = |_: &str| 40u8;
    let output = |item: &str| investment(item, 100.0, 1e12, &["mining"], &level_of, &price, &xp_rate, &rate).expect("ore").units_per_tick * INVESTMENT_HORIZON_TICKS;
    let wanted = |item: &str| (0.75 * output(item)) as i64;
    let bids = vec![bid("coal", 100.0, wanted("coal"), false), bid("copper_ore", 100.0, wanted("copper_ore"), false)];
    let ctx = context(&bids, &[]);
    let bots = vec![bot("a1", &[("mining", 40)]), bot("a2", &[("mining", 40)])];
    let moved = rebalance(&bots, &ctx);
    assert!(moved.iter().all(|row| row.pick.kind == "miner" && row.pick.value_6000 > 0.0), "{moved:?}");
}

/// Each buyer pays HIS price (review): one offer at 1 M and 64 at 3 000 are
/// worth their sum, about 1.2 M above the cost, never 65 x 1 M.
#[test]
fn each_buyer_pays_his_own_price() {
    let level_of = |_: &str| 99u8;
    let mut ladder = vec![(1_000_000.0, 1.0)];
    ladder.extend(std::iter::repeat_n((3_000.0, 1.0), 64));
    let sum = investment_ladder("amulet_of_power", &ladder, &["magic"], &level_of, &price, &xp_rate, &rate).expect("ladder");
    let naive = investment("amulet_of_power", 1_000_000.0, 65.0, &["magic"], &level_of, &price, &xp_rate, &rate).expect("naive");
    let paid: f64 = sum.ladder.iter().map(|(unit, units)| (unit - sum.cost_per_unit) * units).sum();
    assert!(paid < 1_300_000.0 && paid > 1_100_000.0, "1 M + 64 x 3 000 minus costs: {paid}");
    assert!(sum.value_6000 * 10.0 < naive.value_6000, "the ladder value {sum:?} against 65 x 1 M {naive:?}");
    // Through the kind: a bid carrying its ladder is worth less than one that claims the best price for all.
    let flat = vec![bid("amulet_of_power", 1_000_000.0, 65, false)];
    let laddered = vec![UnservedBid { ladder: ladder.iter().map(|(unit, units)| (*unit, *units as i64)).collect(), ..bid("amulet_of_power", 1_000_000.0, 65, false) }];
    let who = bot("m1", &[("magic", 60)]);
    let flat_value = kind_value(&who, "mage", &context(&flat, &[])).value_6000;
    let laddered_value = kind_value(&who, "mage", &context(&laddered, &[])).value_6000;
    assert!(laddered_value * 10.0 < flat_value, "{laddered_value} against {flat_value}");
}

fn context_with<'a>(bids: &'a [UnservedBid], roster: &'a [FleetBot], targets: &'a [(String, String, u8)]) -> ValueContext<'a> {
    ValueContext { roster: Some(roster), targets, ..context(bids, &[]) }
}

/// Live 2026-11-21: mages were given the enchant of the amulet of power
/// while nobody had crafting 70 for the diamond amulet (0 amulets made).
/// Coverage is binary: with nobody at the level and nobody aiming
/// exactly at it, the mage's value of the bid falls and the crafter (66,
/// the closest) gets it; a bot AT 70, or one aiming at crafting 70
/// within the horizon, covers the step and the mage's value returns.
/// Low crafters on the kind that aim at nothing exact cover nothing.
#[test]
fn an_uncovered_upstream_step_takes_the_value_from_the_last_step() {
    let steps = crate::chain_coverage::chain_steps("amulet_of_power");
    assert!(steps.iter().any(|(skill, level)| skill == "crafting" && *level >= 70) && steps.iter().any(|(skill, _)| skill == "magic"), "{steps:?}");
    let bids = vec![bid("amulet_of_power", 1_000_000.0, 66, false)];
    let mage = bot("m0", &[("magic", 60), ("crafting", 30)]);
    let crafter = bot("c0", &[("magic", 60), ("crafting", 66)]);
    let unread = context(&bids, &[]);
    let support = holding("support", "smith", &[("smithing", 99), ("mining", 99), ("runecraft", 99)]);
    // Eight crafters on the kind at 40 to 69, none aiming at 70 exactly.
    let mut nobody = vec![support.clone(), holding("other_mage", "mage", &[("magic", 70), ("crafting", 30)])];
    nobody.extend((0..8).map(|index| holding(&format!("low{index}"), "crafter", &[("crafting", 40 + index * 3)])));
    let uncovered = context_with(&bids, &nobody, &[]);
    let mage_before = kind_value(&mage, "mage", &unread).value_6000;
    let mage_uncovered = kind_value(&mage, "mage", &uncovered).value_6000;
    assert!(mage_uncovered < mage_before / 10.0, "uncovered: mage {mage_uncovered} against {mage_before}");
    let crafter_before = kind_value(&crafter, "crafter", &unread).value_6000;
    let crafter_uncovered = kind_value(&crafter, "crafter", &uncovered).value_6000;
    assert!(crafter_uncovered > crafter_before, "the uncovered step takes the value: {crafter_uncovered} against {crafter_before}");
    assert!(crafter_uncovered > mage_uncovered, "crafter {crafter_uncovered} against mage {mage_uncovered}");
    // A bot at 70 covers it.
    let mut at_level = nobody.clone();
    at_level.push(holding("master", "crafter", &[("crafting", 70)]));
    let covered = kind_value(&mage, "mage", &context_with(&bids, &at_level, &[])).value_6000;
    assert!(covered > mage_uncovered * 5.0, "covered by a level-70 bot: mage {covered} against {mage_uncovered}");
    // A bot aiming exactly at crafting 70 within the horizon covers it too.
    let aims = vec![("low3".to_string(), "crafting".to_string(), 70u8)];
    let aiming = kind_value(&mage, "mage", &context_with(&bids, &nobody, &aims)).value_6000;
    assert!(aiming > mage_uncovered * 5.0, "covered by a bot aiming at 70: mage {aiming} against {mage_uncovered}");
    // A target short of the exact level covers nothing.
    let near_miss = vec![("low3".to_string(), "crafting".to_string(), 60u8)];
    assert!(kind_value(&mage, "mage", &context_with(&bids, &nobody, &near_miss)).value_6000 < mage_before / 10.0);
    // Protected accounts cover nothing: a maxed protected bot changes no value.
    let with_protected = vec![holding("Zezima", "overall", &[("crafting", 99), ("magic", 99)])];
    assert!(kind_value(&mage, "mage", &context_with(&bids, &with_protected, &[])).value_6000 < mage_before / 10.0, "a protected account covers no step");
}
