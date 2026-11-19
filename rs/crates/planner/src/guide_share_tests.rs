//! Shared demand (plan équilibre step 2): the claims of other producers
//! and the asks resting at or under the price sell before a new
//! producer; a bot's pick reprices a row on the claims of the others.

use super::SharedDemand;
use crate::guide_engine::{ahead_units, ladder_value, market_depth, sale_ladder};
use crate::guide_model::{Confidence, Family, FlowLine, MoneyRow, Outlet, PriceBasis, Prereqs, Quote, ShopNeed};

/// Mithril bars, live 2026-11-19: traded at 1 301, the last five fills
/// moved 100 bars, no bid, no shop paying more than 300.
fn mithril(claimed: f64) -> Quote {
    Quote { price: 1301.0, traded: true, median: 1301.0, fills: 5, fill_units: 100.0, claimed, ..Quote::default() }
}

fn shop(price: f64) -> Option<(f64, ShopNeed)> {
    Some((price, ShopNeed::default()))
}

/// Two bots on the same object only see their share: the first sees the
/// 100 bars the market took recently; once it claims 60 of them, the
/// second sees 40 at the price, the rest goes to the shop.
#[test]
fn two_producers_on_one_object_each_see_their_share() {
    let alone = sale_ladder(&mithril(0.0), shop(300.0));
    assert!((ladder_value(&alone, 100.0) - 100.0 * 1301.0).abs() < 1e-6, "{alone:?}");
    assert!((market_depth(&alone) - 100.0).abs() < 1e-9);
    let second = sale_ladder(&mithril(60.0), shop(300.0));
    assert!((market_depth(&second) - 40.0).abs() < 1e-9, "60 claimed by the first: {second:?}");
    assert!((ladder_value(&second, 100.0) - (40.0 * 1301.0 + 60.0 * 300.0)).abs() < 1e-6, "{second:?}");
    let third = sale_ladder(&mithril(100.0), None);
    assert_eq!(market_depth(&third), 0.0, "everything claimed: nothing left");
    assert_eq!(ladder_value(&third, 100.0), 0.0, "no shop: worth nothing");
}

/// Asks already resting at or under the price sell first: 30 at 1 200
/// and 20 at 1 301 take half the recent volume; an ask above the price
/// (1 500) waits behind the new producer.
#[test]
fn resting_asks_at_or_under_the_price_reduce_the_absorbed_volume() {
    let quote = Quote { asks: vec![(1200.0, 30.0), (1301.0, 20.0), (1500.0, 500.0)], ..mithril(0.0) };
    assert!((ahead_units(&quote) - 50.0).abs() < 1e-9);
    let ladder = sale_ladder(&quote, None);
    assert!((market_depth(&ladder) - 50.0).abs() < 1e-9, "{ladder:?}");
    assert!((ladder_value(&ladder, 100.0) - 50.0 * 1301.0).abs() < 1e-6);
    // The supply ahead serves the best buyers first: a bid above the
    // price is taken before the current price's volume.
    let lifted = Quote { bids: vec![(1400.0, 10.0)], claimed: 15.0, ..mithril(0.0) };
    let ladder = sale_ladder(&lifted, None);
    assert!((market_depth(&ladder) - 95.0).abs() < 1e-9, "{ladder:?}");
    assert!((ladder.first().map(|level| level.price).unwrap_or(0.0) - 1301.0).abs() < 1e-9, "the bid went to the claims: {ladder:?}");
}

/// A never-traded object: the claims take its resting bids first.
#[test]
fn claims_take_the_bids_of_a_never_traded_object() {
    let quote = Quote { price: 1.0, traded: false, bids: vec![(90.0, 5.0), (80.0, 10.0)], claimed: 8.0, ..Quote::default() };
    let ladder = sale_ladder(&quote, None);
    assert!((market_depth(&ladder) - 7.0).abs() < 1e-9, "{ladder:?}");
    assert!((ladder_value(&ladder, 7.0) - 7.0 * 80.0).abs() < 1e-6);
}

fn line(item: &str, cause: &str, per_6000: f64, price: f64) -> FlowLine {
    FlowLine { item: item.to_string(), cause: cause.to_string(), per_6000, unit_price: price, value_6000: per_6000 * price, outlet: Outlet::Market, traded: true, basis: PriceBasis::Traded }
}

/// The guide's row of mithril bars: 50 a window at 1 301, 100 mithril
/// ores and 200 coal of stock (D5) at 400 and 30.
fn mithril_row() -> MoneyRow {
    let flows = vec![line("mithril_bar", "product", 50.0, 1301.0), line("mithril_ore", "input", -50.0, 400.0), line("coal", "input", -200.0, 30.0)];
    let revenue = 50.0 * 1301.0;
    let cost = 50.0 * 400.0 + 200.0 * 30.0;
    MoneyRow {
        family: Family::Recipe,
        method: "smelt:mithril_bar".to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 56,
        prereqs: Prereqs::default(),
        flows,
        other_drops: Default::default(),
        revenue_6000: revenue,
        cost_6000: cost,
        profit_6000: revenue - cost,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        priced: true,
    }
}

/// A claim lowers another bot's valuation: with no claim the row keeps
/// the guide's profit; once another smelter claims 80 of the 100 bars,
/// this bot sells 20 at the price and 30 to the shop; with every bar
/// claimed and no shop, the row loses its inputs.
#[test]
fn a_claim_lowers_another_bots_valuation() {
    let row = mithril_row();
    let open = |item: &str| (item == "mithril_bar").then(|| mithril(0.0));
    let no_shop = |_: &str| None;
    let free = SharedDemand::new(&open, &no_shop);
    assert!((free.profit(&row) - row.profit_6000).abs() < 1e-6, "no claim: the guide's own value");
    let claimed = |item: &str| (item == "mithril_bar").then(|| mithril(80.0));
    let shop_300 = |item: &str| if item == "mithril_bar" { shop(300.0) } else { None };
    let shared = SharedDemand::new(&claimed, &shop_300);
    let expected = 20.0 * 1301.0 + 30.0 * 300.0 - (50.0 * 400.0 + 200.0 * 30.0);
    assert!((shared.profit(&row) - expected).abs() < 1e-6, "{} vs {expected}", shared.profit(&row));
    let gone = |item: &str| (item == "mithril_bar").then(|| mithril(100.0));
    let taken = SharedDemand::new(&gone, &no_shop);
    assert!(taken.profit(&row) < 0.0, "every bar claimed: a losing line ({})", taken.profit(&row));
}
