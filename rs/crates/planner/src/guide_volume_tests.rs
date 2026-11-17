//! Decisions utilisateur 2026-11-17 on fabricated worlds: the current
//! price holds for the units its fills moved, whatever their count (§1),
//! and a training method that spends only consumables gets a market line
//! beside its autonomous one (§2).

use super::{answer, level, row, training_row, Fixture};
use crate::guide_model::{Family, GuideFilter, Outlet, PriceBasis, Quote, Route, Supply};
use crate::guide_money::money_guide;
use crate::guide_training::training_guide;

/// §1: the current price (median of the last five fills) holds for the
/// VOLUME those fills moved (`Quote::fill_units`), even on five fills:
/// past it a sale hits the remaining bids for their depth, then the F2P
/// shop (Q3), else is worth nothing. Leather cowl, live 2026-11-17: five
/// fills of one unit around 462 price 5 units of a 988-unit window at
/// 462, never 988.
#[test]
fn current_price_holds_for_the_recent_fill_volume_only() {
    let mut fixture = Fixture::new();
    fixture.quotes.insert("leather_cowl", Quote { price: 462.0, traded: true, median: 462.0, fills: 5, fill_units: 5.0, bids: vec![(30.0, 100.0)], asks: Vec::new() });
    fixture.npc.insert("leather_cowl", 10.0);
    let (five, eight, window) = fixture.run(|engine| (engine.sale_at("leather_cowl", 5.0), engine.sale_at("leather_cowl", 8.0), engine.sale_at("leather_cowl", 988.0)));
    assert!((five.price - 462.0).abs() < 1e-9 && five.basis == PriceBasis::Traded, "the traded volume: {five:?}");
    assert!((eight.price - (5.0 * 462.0 + 3.0 * 30.0) / 8.0).abs() < 1e-9, "{eight:?}");
    assert_eq!(eight.basis, PriceBasis::Thin, "past the traded volume, the current price still the majority");
    let expected = (5.0 * 462.0 + 100.0 * 30.0 + 883.0 * 10.0) / 988.0;
    assert!((window.price - expected).abs() < 1e-9, "5 at 462, 100 at the bid, 883 at the shop: {} vs {expected}", window.price);
    assert_eq!((window.basis, window.outlet), (PriceBasis::BookShop, Outlet::Shop));
    fixture.npc.clear();
    let window = fixture.run(|engine| engine.sale_at("leather_cowl", 988.0));
    assert!((window.price - (5.0 * 462.0 + 100.0 * 30.0) / 988.0).abs() < 1e-9, "no outlet past the bids: {}", window.price);
    assert_eq!(window.basis, PriceBasis::BookNone);
    // Five fills that moved the whole volume: every unit at the price.
    fixture.quotes.insert("leather_cowl", Quote { price: 462.0, traded: true, median: 462.0, fills: 5, fill_units: 1000.0, ..Quote::default() });
    let window = fixture.run(|engine| engine.sale_at("leather_cowl", 988.0));
    assert!((window.price - 462.0).abs() < 1e-9 && window.basis == PriceBasis::Traded, "{window:?}");
    // A money line (and so D17) counts the traded volume only: 100 coal
    // a window, five fills of one coal each, no bid, no shop.
    fixture.quotes.insert("coal", Quote { price: 15.0, traded: true, median: 15.0, fills: 5, fill_units: 5.0, ..Quote::default() });
    let (rows, best) = fixture.run(|engine| (money_guide(engine, &level(30)).0, crate::guide_money::best_accessible_profit(engine, 30, &[])));
    let coal = row(&rows, "gather:mining/coal", Family::Farm);
    assert!((coal.profit_6000 - 5.0 * 15.0).abs() < 1e-9, "5 coal at 15, 95 worth nothing: {}", coal.profit_6000);
    let top = rows.iter().filter(|row| row.accessible).map(|row| row.profit_6000).fold(0.0, f64::max);
    assert_eq!(best, Some(top), "D17 reads the same rows");
}

/// §1, purchase side: the current price gives no depth to a purchase;
/// only the asks (each unit at max(current price, ask)) and the shops'
/// window do. Five fills of 5 000 units do not sell a single unit
/// nobody offers.
#[test]
fn current_price_gives_no_purchase_depth() {
    let mut fixture = Fixture::new();
    fixture.quotes.insert("coal", Quote { price: 15.0, traded: true, median: 15.0, fills: 5, fill_units: 5000.0, bids: Vec::new(), asks: vec![(10.0, 50.0)] });
    let (fifty, more) = fixture.run(|engine| (engine.buy_at("coal", 50.0), engine.buy_at("coal", 200.0)));
    assert!(fifty.available && (fifty.price - 15.0).abs() < 1e-9, "max(current price, ask): {fifty:?}");
    assert!(!more.available, "past the asks, no shop: not for sale: {more:?}");
}

fn teleport_world() -> Fixture {
    let mut fixture = Fixture::new();
    let (fight, _) = crate::guide_chain::sources("lawrune").into_iter().find(|(method, _)| method.starts_with("fight:")).expect("a fight drops law runes");
    let fight: &'static str = Box::leak(fight.into_boxed_str());
    // Two law runes a window off the fight: the autonomous line farms them.
    fixture.yields.insert(fight, answer(&[("lawrune", "loot", 2.0)], &[]));
    fixture.yields.insert("teleport:varrock_teleport", answer(&[("lawrune", "rune", -100.0), ("airrune", "rune", -300.0), ("firerune", "rune", -100.0)], &[("cast", "varrock_teleport", 100.0)]));
    fixture.prices.extend([("lawrune", 200.0), ("airrune", 5.0), ("firerune", 5.0)]);
    fixture
}

/// §2: a method that spends only consumables (teleport: law, air, fire
/// runes) gets a MARKET line beside its autonomous one: every consumable
/// bought at its volume (asks, shops; the purchase rule), the coins
/// converted to time at the best accessible money line (D17), as every
/// market line.
#[test]
fn consumable_only_method_has_a_market_line() {
    let fixture = teleport_world();
    let filter = GuideFilter { skill: Some("magic".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let autonomous = training_row(&rows, "teleport:varrock_teleport", Supply::Autonomous);
    assert_eq!(autonomous.routes.iter().find(|route| route.item == "lawrune").map(|route| route.route), Some(Route::Farm), "{:?}", autonomous.routes);
    let market = training_row(&rows, "teleport:varrock_teleport", Supply::Market);
    assert!(market.blocked.is_empty() && market.accessible, "{market:?}");
    let law = market.flows.iter().find(|line| line.item == "lawrune").expect("law runes bought");
    assert!(law.per_6000 < 0.0 && (law.unit_price - 200.0).abs() < 1e-9, "{law:?}");
    let spent = 100.0 * 200.0 + 300.0 * 5.0 + 100.0 * 5.0;
    assert!((market.coins_6000 + spent).abs() < 1e-6, "{}", market.coins_6000);
    let best = fixture.run(|engine| crate::guide_money::best_accessible_profit(engine, 30, &[])).expect("a money line earns");
    assert!((market.converted_ticks_6000 - spent / (best / 6000.0)).abs() < 1e-6, "D17: {}", market.converted_ticks_6000);
    assert!((market.effective_xp_6000 - market.xp_6000 * 6000.0 / (6000.0 + market.converted_ticks_6000)).abs() < 1e-6);
    assert!(market.effective_xp_6000 > autonomous.effective_xp_6000, "buying beats 3000 ticks of fighting a rune");
    assert!(!market.prereqs.ironman, "a market line: no Ironman");
}

/// §2: the market line buys at the VOLUME available: law runes no ask
/// covers and no shop sells block it (reason, no rate); the autonomous
/// line still farms them. A method whose autonomous line already buys
/// every consumable (nothing makes any) gets no copy of it.
#[test]
fn consumable_market_line_is_blocked_past_the_available_volume() {
    let mut fixture = teleport_world();
    fixture.prices.remove("lawrune");
    fixture.books.insert("lawrune", (Vec::new(), vec![(24.0, 12.0)]));
    let filter = GuideFilter { skill: Some("magic".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let market = training_row(&rows, "teleport:varrock_teleport", Supply::Market);
    assert!(!market.accessible && market.blocked.iter().any(|reason| reason.contains("lawrune")), "{market:?}");
    assert_eq!(market.effective_xp_6000, 0.0, "a blocked line has no rate");
    assert!(training_row(&rows, "teleport:varrock_teleport", Supply::Autonomous).blocked.is_empty(), "the drop still feeds the autonomous line");
    // Nothing makes any rune: the autonomous line buys them all, the
    // market line would be its copy.
    let mut fixture = teleport_world();
    fixture.yields.retain(|method, _| !method.starts_with("fight:"));
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let lines: Vec<Supply> = rows.iter().filter(|row| row.method == "teleport:varrock_teleport").map(|row| row.supply).collect();
    assert_eq!(lines, vec![Supply::Autonomous], "no copy");
}
