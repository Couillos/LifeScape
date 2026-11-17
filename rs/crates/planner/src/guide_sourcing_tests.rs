//! Decisions utilisateur 2026-11-16 on fabricated worlds: consumables
//! produced in autonomous mode (§1), finite shop stock (§2), each chain
//! input the cheaper way (§3), one line per key (§4).

use super::{answer, level, reachable, row, training_row, why_not, Fixture};
use crate::guide_chain::{chain_unit, sources, Sourcing, TimePrice};
use crate::guide_model::{Family, GuideFilter, MeasuredKey, PriceBasis, Route, Supply};
use crate::guide_money::money_guide;
use crate::guide_training::training_guide;

/// The activity the game data names as a source of `item` (a fight's
/// drop, a production leg), never a hand-written name.
fn source_of(item: &str, prefix: &str) -> (String, String) {
    sources(item).into_iter().find(|(method, _)| method.starts_with(prefix)).unwrap_or_else(|| panic!("a {prefix} source of {item}: {:?}", sources(item)))
}

/// §1: a consumable the bot can produce (feathers: a chicken fight's
/// drop) is farmed in autonomous mode, its ticks counted like an input's
/// (no purchase line); one no activity of this world yields (fishing bait:
/// its sources, a fight dropping it, have no yield here) stays bought.
#[test]
fn autonomous_mode_farms_productible_consumables_and_buys_the_rest() {
    let mut fixture = Fixture::new();
    let (fight, style) = source_of("feather", "fight:");
    let fight: &'static str = Box::leak(fight.into_boxed_str());
    assert_eq!(style, crate::guide_chain::SOURCE_FIGHT_STYLE);
    fixture.yields.insert(fight, answer(&[("feather", "loot", 600.0), ("bones", "loot", 60.0)], &[]));
    fixture.yields.insert("gather:fishing/raw_trout", answer(&[("raw_trout", "product", 300.0), ("feather", "bait", -300.0)], &[("product", "raw_trout", 300.0)]));
    fixture.yields.insert("gather:fishing/raw_sardine", answer(&[("raw_sardine", "product", 300.0), ("fishing_bait", "bait", -300.0)], &[("product", "raw_sardine", 300.0)]));
    fixture.prices.extend([("feather", 3.0), ("raw_trout", 20.0), ("raw_sardine", 8.0), ("fishing_bait", 4.0), ("bones", 50.0)]);
    let filter = GuideFilter { skill: Some("fishing".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let trout = training_row(&rows, "gather:fishing/raw_trout", Supply::Autonomous);
    assert!(trout.priced && trout.prereqs.sources.is_empty(), "{:?}", trout.prereqs);
    assert!(!trout.flows.iter().any(|line| line.item == "feather" && line.per_6000 < 0.0), "feathers farmed, never bought: {:?}", trout.flows);
    let route = trout.routes.iter().find(|route| route.item == "feather").expect("feather route");
    assert_eq!((route.route, route.source.as_str(), route.cause.as_str()), (Route::Farm, fight, "bait"));
    // 300 feathers a window, 10 ticks each: 3000 more ticks per 6000.
    let scale = 6000.0 / 9000.0;
    assert!((trout.xp_6000 - 300.0 * 50.0 * scale).abs() < 1e-6, "{}", trout.xp_6000);
    // The fight's other drops ride the line (30 bones per 3000 ticks).
    assert!((trout.coins_6000 - (300.0 * 20.0 + 30.0 * 50.0) * scale).abs() < 1e-6, "{}", trout.coins_6000);
    assert!(sources("fishing_bait").iter().all(|(method, _)| !fixture.yields.contains_key(method.as_str())), "no yield makes bait here");
    let sardine = training_row(&rows, "gather:fishing/raw_sardine", Supply::Autonomous);
    let bait = sardine.flows.iter().find(|line| line.item == "fishing_bait").expect("bait bought");
    assert!(bait.per_6000 < 0.0 && (bait.unit_price - 4.0).abs() < 1e-9, "{bait:?}");
    assert_eq!(sardine.routes.iter().find(|route| route.item == "fishing_bait").map(|route| route.route), Some(Route::Buy));
}

/// §2: a shop sells its base stock and its restock of the window, each
/// unit at the price of the stock left (cheapest first, merged with the
/// asks); past them the purchase is not covered and the row needing it
/// names the condition (D22), as past the asks' depth.
#[test]
fn shop_stock_is_finite_and_its_price_rises() {
    let mut fixture = Fixture::new();
    fixture.prices.remove("coal");
    fixture.shop_ladders.insert("coal", vec![(12.0, 100.0), (15.0, 150.0)]);
    let (two_hundred, three_hundred) = fixture.run(|engine| (engine.buy_at("coal", 200.0), engine.buy_at("coal", 300.0)));
    assert!(two_hundred.available && (two_hundred.price - (100.0 * 12.0 + 100.0 * 15.0) / 200.0).abs() < 1e-9, "{two_hundred:?}");
    assert_eq!(two_hundred.basis, PriceBasis::Shop);
    assert!(!three_hundred.available, "the window holds 250 units: {three_hundred:?}");
    let need = fixture.run(|engine| engine.market_need("coal", "input", 300.0, 19.0, three_hundred.priced));
    assert!(need.shops == 250.0 && need.asks == 0.0 && need.priced, "the condition names the shops' window: {need:?}");
    // Asks and shop steps walk together, cheapest first.
    fixture.books.insert("coal", (Vec::new(), vec![(10.0, 20.0), (20.0, 1000.0)]));
    let mixed = fixture.run(|engine| engine.buy_at("coal", 400.0));
    let expected = (20.0 * 10.0 + 100.0 * 12.0 + 150.0 * 15.0 + 130.0 * 20.0) / 400.0;
    assert!(mixed.available && (mixed.price - expected).abs() < 1e-9, "{mixed:?} vs {expected}");
    // A market row needing more coal than asks + shop hold: no bot without
    // coal can buy a lot (D22: a condition, never a block).
    fixture.books.clear();
    fixture.shop_ladders.insert("coal", vec![(12.0, 10.0)]);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(market.priced && why_not(market, &level(30)).contains("coal"), "{market:?}");
}

/// §3 (D16 revised): each input of a chain takes the cheaper way:
/// farming (its ticks at the bot's D17 price, minus its other flows) or
/// buying (D15 at its volume); the chain names the way of each input.
#[test]
fn chain_takes_the_cheaper_way_per_input() {
    let mut fixture = Fixture::new();
    // D17 at 1500 coins per 6000 ticks: a coal (60 ticks) costs 15 farmed,
    // an iron ore (50 ticks) 12.5.
    let price = TimePrice::new(1500.0);
    fixture.shop.insert("coal", 5.0);
    fixture.prices.insert("iron_ore", 20.0);
    let unit = fixture.run(|engine| chain_unit(engine, "smelt:steel_bar", "", "steel_bar", 30, 0, &[], Sourcing::Cheapest(price))).expect("chain");
    let way = |item: &str| unit.routes.iter().find(|route| route.item == item).unwrap_or_else(|| panic!("{item} route: {:?}", unit.routes)).clone();
    let coal = way("coal");
    assert_eq!(coal.route, Route::Buy, "{coal:?}");
    assert!((coal.buy_coins_per_unit.expect("priced") - 5.0).abs() < 1e-9 && (coal.farm_coins_per_unit.expect("farmable") - 15.0).abs() < 1e-9, "{coal:?}");
    let iron = way("iron_ore");
    assert_eq!(iron.route, Route::Farm, "{iron:?}");
    assert!((iron.farm_coins_per_unit.expect("farmable") - 12.5).abs() < 1e-9, "{iron:?}");
    assert!((unit.ticks_per_unit - (60.0 + 50.0)).abs() < 1e-9, "coal bought: no mining ticks: {}", unit.ticks_per_unit);
    assert!((unit.value_consumed + 2.0 * 5.0).abs() < 1e-9, "{}", unit.value_consumed);
    // Coal dearer than its mining (never traded, the shop at 40): farmed
    // again (a tie farms: buying must be cheaper).
    fixture.prices.remove("coal");
    fixture.shop.insert("coal", 40.0);
    let unit = fixture.run(|engine| chain_unit(engine, "smelt:steel_bar", "", "steel_bar", 30, 0, &[], Sourcing::Cheapest(price))).expect("chain");
    assert_eq!(unit.routes.iter().find(|route| route.item == "coal").map(|route| route.route), Some(Route::Farm));
    assert!((unit.ticks_per_unit - (60.0 + 50.0 + 2.0 * 60.0)).abs() < 1e-9);
    // The money guide's chain line shows the ways (D17 from the best
    // line outside the chains: iron mined at 20, 0.4 coin a tick).
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let chain = row(&rows, "smelt:steel_bar", Family::Chain);
    assert!(chain.routes.iter().any(|route| route.item == "coal") && chain.routes.iter().any(|route| route.item == "iron_ore"), "{:?}", chain.routes);
}

/// §4: one line per key (method, style, site, tool, band, mode): a
/// measured site whose own key holds no measure (the estimator read a
/// roll-up or the prior) is no line, never a copy of the roll-up; a key
/// listed twice is one line.
#[test]
fn a_measured_site_without_its_own_data_is_no_line() {
    let mut fixture = Fixture::new();
    let measured = |site: (i64, i64, i64)| MeasuredKey { method: "gather:mining/coal".to_string(), style: String::new(), site: Some(site), tool: crate::guide_methods::tool_at("gather:mining/coal", "", 30), band: 26 };
    let (own, copy) = ((3000, 3000, 0), (3100, 3100, 0));
    fixture.measured = vec![measured(own), measured(own), measured(copy)];
    fixture.site_reads.insert(copy, "AllSites");
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let coal: Vec<_> = rows.iter().filter(|row| row.method == "gather:mining/coal").map(|row| row.site).collect();
    assert_eq!(coal, vec![None, Some(own)], "the prior's line and the measured site, once each");
    let filter = GuideFilter { skill: Some("mining".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let coal: Vec<_> = rows.iter().filter(|row| row.method == "gather:mining/coal").map(|row| (row.site, row.supply)).collect();
    assert_eq!(coal, vec![(None, Supply::Autonomous), (Some(own), Supply::Autonomous)], "{coal:?}");
}

/// User 2026-11-16: in AUTONOMOUS mode every drop counts as producible,
/// however rare (law runes off hobgoblins): no rarity threshold; the line
/// shows the slowness honestly (the fight's ticks per rune counted), the
/// market mode buys them.
#[test]
fn a_rare_drop_is_farmed_in_autonomous_mode_however_slow() {
    let mut fixture = Fixture::new();
    let (fight, style) = source_of("lawrune", "fight:");
    let fight: &'static str = Box::leak(fight.into_boxed_str());
    assert_eq!(style, crate::guide_chain::SOURCE_FIGHT_STYLE);
    // Two law runes a window: one every 3000 ticks of fighting.
    fixture.yields.insert(fight, answer(&[("lawrune", "loot", 2.0)], &[]));
    fixture.yields.insert("teleport:varrock_teleport", answer(&[("lawrune", "rune", -100.0)], &[("cast", "varrock_teleport", 100.0)]));
    fixture.prices.insert("lawrune", 200.0);
    let filter = GuideFilter { skill: Some("magic".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let autonomous = training_row(&rows, "teleport:varrock_teleport", Supply::Autonomous);
    assert!(autonomous.priced && autonomous.prereqs.sources.is_empty(), "a rare drop is still a source: {:?}", autonomous.prereqs.sources);
    let route = autonomous.routes.iter().find(|route| route.item == "lawrune").expect("law rune route");
    assert_eq!((route.route, route.source.as_str()), (Route::Farm, fight));
    assert!((route.farm_ticks_per_unit.unwrap_or(0.0) - 3000.0).abs() < 1e-6, "{route:?}");
    // 100 casts need 300 000 ticks of fighting: the XP shows it (the
    // casts alone would take the whole window).
    let cast_xp = crate::unit_xp::derived_xp("teleport:varrock_teleport", "", &[("cast".to_string(), "varrock_teleport".to_string(), 100.0)], 30).into_iter().find(|(skill, _)| skill == "magic").map(|(_, xp)| xp).unwrap_or(0.0);
    let expected = cast_xp * 6000.0 / (6000.0 + 100.0 * 3000.0);
    assert!(cast_xp > 0.0 && (autonomous.xp_6000 - expected).abs() < 1e-6 * cast_xp, "{} vs {expected}", autonomous.xp_6000);
    // Buying them: the money guide's line buys its runes.
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let cast = row(&rows, "teleport:varrock_teleport", Family::Farm);
    assert!(cast.flows.iter().any(|line| line.item == "lawrune" && line.per_6000 < 0.0 && (line.unit_price - 200.0).abs() < 1e-9), "{:?}", cast.flows);
}

/// Q7 (user 2026-11-16): a shop behind a condition (an F2P quest, the
/// Champions' Guild's quest points) is in the purchase window AND the
/// sale outlets, its condition a prerequisite of the row that trades
/// with it: a bot without it cannot work the row; one with it can.
#[test]
fn a_conditioned_shop_is_a_prerequisite_of_its_row() {
    use crate::guide_model::ShopNeed;
    let mut fixture = Fixture::new();
    fixture.prices.remove("coal");
    fixture.prices.remove("iron_ore");
    fixture.shop_ladders.insert("coal", vec![(12.0, 1e6)]);
    fixture.shop_ladders.insert("iron_ore", vec![(9.0, 1e6)]);
    fixture.shop_needs.insert("coal", ShopNeed { quest: Some("dragon"), quest_points: 0 });
    fixture.shop_needs.insert("iron_ore", ShopNeed { quest: None, quest_points: 32 });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(market.priced, "{market:?}");
    assert!(market.prereqs.quests.contains(&"dragon".to_string()) && market.prereqs.quest_points == 32, "{:?}", market.prereqs);
    assert!(!reachable(market, &level(30)), "neither the quest nor the points");
    let dragon_only = GuideFilter { quests_done: vec!["dragon".to_string()], ..level(30) };
    assert!(!crate::guide_prereq::passes(&market.prereqs, &dragon_only), "Dragon Slayer alone grants fewer than 32 points");
    let every_quest: Vec<String> = lifescape_gamedata::quest_requirements::all_f2p_quests().into_iter().map(|quest| quest.key).collect();
    let veteran = GuideFilter { quests_done: every_quest, ..level(30) };
    assert!(crate::guide_prereq::passes(&market.prereqs, &veteran), "every F2P quest: the quest and the points");
    let (rows, _) = fixture.run(|engine| money_guide(engine, &veteran));
    assert!(reachable(row(&rows, "smelt:steel_bar", Family::Market), &veteran));
    // The sale side: coal sold to a shop behind the guild door.
    let mut fixture = Fixture::new();
    fixture.npc.insert("coal", 70.0);
    fixture.shop_needs.insert("coal", ShopNeed { quest: None, quest_points: 32 });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let coal = row(&rows, "gather:mining/coal", Family::Farm);
    assert!((coal.profit_6000 - 100.0 * 70.0).abs() < 1e-9 && coal.prereqs.quest_points == 32, "{} {:?}", coal.profit_6000, coal.prereqs);
    assert!(!reachable(coal, &level(30)));
    // A recipe values its stock at the sale price: it trades with nobody
    // for its inputs (its product, steel, sells on the market).
    let recipe = row(&rows, "smelt:steel_bar", Family::Recipe);
    assert_eq!(recipe.prereqs.quest_points, 0, "{:?}", recipe.prereqs);
}
