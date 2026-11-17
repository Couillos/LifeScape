//! D22 (user 2026-11-17) on fabricated worlds: the guides block nothing.
//! Every row keeps its economy at the current price and carries its
//! conditions; each bot judges a row by `guide_feasible::feasible_for`
//! (its stock, its levels, the market of the moment).

use super::{answer, judged, level, reachable, row, why_not, Fixture};
use crate::guide_feasible::{conditions, Condition, Feasibility};
use crate::guide_model::{Family, GuideFilter, Quote};
use crate::guide_money::money_guide;

/// The alchemy world: iron platebodies alched with nature runes (traded,
/// absorbed by bids: no ask rests) and fire runes (offered). The
/// platebody itself was never traded and nobody offers one.
fn alchemy_world() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.yields.insert(
        "alch:iron_platebody",
        answer(&[("coins", "product", 1000.0 * 600.0), ("iron_platebody", "input", -1000.0), ("naturerune", "rune", -1000.0), ("firerune", "rune", -5000.0)], &[("input", "iron_platebody", 1000.0)]),
    );
    fixture.quotes.insert("naturerune", Quote { price: 200.0, traded: true, median: 200.0, fills: 5, fill_units: 40.0, ..Quote::default() });
    fixture.prices.insert("firerune", 5.0);
    fixture.npc.insert("iron_platebody", 300.0);
    fixture
}

fn holding(items: &[(&str, f64)]) -> GuideFilter {
    GuideFilter { level: Some(60), held: items.iter().map(|(item, units)| (item.to_string(), *units)).collect(), ..GuideFilter::default() }
}

/// No row is blocked globally: the alchemy rows keep their economy at the
/// current price although no nature rune is offered (before D22: "asks
/// hold 0", no profit, never accessible); the market row of an object
/// nothing prices stays listed, unpriced, its condition saying why.
#[test]
fn no_row_is_blocked_globally() {
    let fixture = alchemy_world();
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(60)));
    let recipe = row(&rows, "alch:iron_platebody", Family::Recipe);
    // 600 000 coins − 1000 plates at the shop's 300 − 1000 natures at 200
    // − 5000 fires at 5: the window's economy at the current prices.
    assert!(recipe.priced && (recipe.profit_6000 - 75_000.0).abs() < 1e-6, "{recipe:?}");
    let nature = recipe.prereqs.market.iter().find(|need| need.item == "naturerune").expect("nature rune condition");
    assert_eq!((nature.asks, nature.recent, nature.shops, nature.per_lot, nature.priced), (0.0, 40.0, 0.0, 28.0, true), "{nature:?}");
    let market = row(&rows, "alch:iron_platebody", Family::Market);
    assert!(!market.priced && market.profit_6000 == 0.0, "nothing prices a plate to buy: {market:?}");
    assert!(conditions(&market.prereqs).iter().any(|condition| matches!(condition, Condition::Market(need) if need.item == "iron_platebody" && !need.priced)));
    // Every family of the activity is listed, priced rows first.
    let first_unpriced = rows.iter().position(|row| !row.priced).expect("an unpriced row");
    assert!(rows[first_unpriced..].iter().all(|row| !row.priced), "unpriced rows rank last");
}

/// Alchemy for a bot holding its runes and objects: feasible; for a bot
/// with objects and no nature rune (none offered): infeasible, the reason
/// named; for a bot with runes and no object: infeasible on its stock.
#[test]
fn alchemy_is_feasible_for_the_bot_holding_runes_and_objects() {
    let fixture = alchemy_world();
    let stocked = holding(&[("iron_platebody", 40.0), ("naturerune", 40.0), ("firerune", 200.0)]);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &stocked));
    let recipe = row(&rows, "alch:iron_platebody", Family::Recipe);
    assert_eq!(judged(recipe, &stocked), Feasibility::Feasible);
    // Evaluated on its stock: 40 plates of the window's 1000.
    assert!((recipe.profit_6000 - 75_000.0 * 40.0 / 1000.0).abs() < 1e-6, "{}", recipe.profit_6000);
    let runeless = holding(&[("iron_platebody", 40.0), ("firerune", 200.0)]);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &runeless));
    let recipe = row(&rows, "alch:iron_platebody", Family::Recipe);
    let why = why_not(recipe, &runeless);
    assert!(why.contains("naturerune") && why.contains("asks 0"), "{why}");
    let plateless = holding(&[("naturerune", 40.0), ("firerune", 200.0)]);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &plateless));
    assert!(why_not(row(&rows, "alch:iron_platebody", Family::Recipe), &plateless).contains("iron_platebody"));
    // Once nature runes are offered, the bot with plates can buy a lot.
    let mut offered = alchemy_world();
    offered.quotes.insert("naturerune", Quote { price: 200.0, traded: true, median: 200.0, fills: 5, fill_units: 40.0, asks: vec![(210.0, 100.0)], ..Quote::default() });
    let (rows, _) = offered.run(|engine| money_guide(engine, &runeless));
    assert!(reachable(row(&rows, "alch:iron_platebody", Family::Recipe), &runeless));
}

/// smith:rune_axe as a chain: the runite is mined where no site has a bank
/// in reach (`danger::NEVER`, game data): the row keeps its economy (no
/// block) but carries the site condition, impossible today for every
/// bot, a maxed one included.
#[test]
fn rune_axe_chain_carries_the_impossible_site_condition() {
    let mut fixture = Fixture::new();
    fixture.yields.insert("gather:mining/runite_ore", answer(&[("runite_ore", "product", 4.0)], &[("product", "runite_ore", 4.0)]));
    fixture.yields.insert("smelt:runite_bar", answer(&[("runite_bar", "product", 100.0), ("runite_ore", "input", -100.0), ("coal", "input", -800.0)], &[("product", "runite_bar", 100.0)]));
    fixture.yields.insert("smith:rune_axe", answer(&[("rune_axe", "product", 100.0), ("runite_bar", "input", -100.0)], &[("product", "rune_axe", 100.0)]));
    fixture.prices.insert("rune_axe", 40_000.0);
    let maxed = level(99);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &maxed));
    let chain = row(&rows, "smith:rune_axe", Family::Chain);
    assert!(chain.priced && chain.profit_6000 > 0.0, "its economy stays: {chain:?}");
    let listed = conditions(&chain.prereqs);
    assert!(listed.contains(&Condition::NoSite), "{listed:?}");
    assert!(listed.iter().any(|condition| condition.impossible() && condition.text().contains("no site within bank reach")));
    let found = judged(chain, &maxed);
    assert_eq!(found.state(), "infeasible", "{found:?}");
    assert!(why_not(chain, &maxed).contains("no site within bank reach"));
    // The catalogue view (each row at its gate) says the same: never
    // "accessible" because no level is asked.
    let (catalogue, _) = fixture.run(|engine| money_guide(engine, &GuideFilter::default()));
    assert!(!reachable(row(&catalogue, "smith:rune_axe", Family::Chain), &GuideFilter::default()));
    // The market row: runite bars nothing prices (never traded, no ask,
    // no shop): listed, unpriced, never a made-up profit.
    let market = row(&rows, "smith:rune_axe", Family::Market);
    assert!(!market.priced && market.profit_6000 == 0.0, "{market:?}");
}
