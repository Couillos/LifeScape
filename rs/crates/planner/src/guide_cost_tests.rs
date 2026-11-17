//! Plan marché M7 : coût de production publié par les guides (ancrage
//! public de la découverte du prix), la ligne cotée la moins chère.

use super::*;
use crate::guide_model::{Confidence, FlowLine, Outlet, PriceBasis, Prereqs};

fn line(item: &str, cause: &str, per_6000: f64, price: f64) -> FlowLine {
    FlowLine { item: item.to_string(), cause: cause.to_string(), per_6000, unit_price: price, value_6000: per_6000 * price, outlet: Outlet::Market, traded: true, basis: PriceBasis::Traded }
}

/// A bronze bar line: `bars` per 6000 ticks from as many copper (10) and
/// tin (12) ores, `cost` per 6000 ticks.
fn bronze(family: Family, bars: f64, cost: f64, priced: bool) -> MoneyRow {
    MoneyRow {
        family,
        method: "smelt:bronze_bar".to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 1,
        prereqs: Prereqs::default(),
        flows: vec![line("bronze_bar", "product", bars, 0.0), line("copper_ore", "input", -bars, 10.0), line("tin_ore", "input", -bars, 12.0)],
        other_drops: Default::default(),
        revenue_6000: 0.0,
        cost_6000: cost,
        profit_6000: -cost,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        priced,
    }
}

#[test]
fn the_cheapest_priced_line_publishes_the_cost() {
    assert_eq!(crate::guide_methods::own_product("smelt:bronze_bar").as_deref(), Some("bronze_bar"));
    let market = bronze(Family::Market, 100.0, 2400.0, true);
    let recipe = bronze(Family::Recipe, 100.0, 2200.0, true);
    assert_eq!(unit_cost(&market, "bronze_bar"), Some(24.0));
    assert_eq!(unit_cost(&market, "copper_ore"), None, "an input is not what the line makes");
    let costs = make_costs(&[market.clone(), recipe]);
    assert_eq!(costs.get("bronze_bar"), Some(&22.0), "the recipe's 22 beats the market line's 24");
    // A line whose purchase nothing prices publishes nothing.
    let unpriced = bronze(Family::Chain, 100.0, 100.0, false);
    assert_eq!(make_costs(&[market.clone(), unpriced]).get("bronze_bar"), Some(&24.0));
    // A harvest (farm line) or a line that consumes nothing: no coin cost.
    assert!(make_costs(&[bronze(Family::Farm, 100.0, 2200.0, true)]).is_empty());
    assert!(make_costs(&[bronze(Family::Recipe, 100.0, 0.0, true)]).is_empty());
    assert!(make_costs(&[bronze(Family::Recipe, 0.0, 2200.0, true)]).is_empty(), "no product made");
}
