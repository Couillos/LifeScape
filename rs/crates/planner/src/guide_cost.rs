#![forbid(unsafe_code)]
//! Production cost the guides publish (plan marché M7, §3.8; plan
//! rendements R4): what making one unit of an object costs on the
//! published money lines, a PUBLIC anchor of the price discovery of an
//! object nobody traded yet (`lifescape_market::discovery`).
//!
//! The cost of one unit is a line's cost per 6000 ticks over the units of
//! its own product it makes per 6000 ticks: a recipe's inputs at their
//! current sale price (D5), a chain's inputs by its cheaper way (farmed
//! at the D17 price of time, or bought, D16), a 100 % market line's
//! inputs bought at the asks and shops (D15), consumables included. Only
//! priced lines count (an input nothing prices leaves the line unpriced:
//! no made-up cost), only lines that consume something (a harvest's cost
//! is its time, not published as coins), the cheapest line wins (the way
//! a competitive producer makes it).

use std::collections::HashMap;

use crate::guide_model::{Family, MoneyRow};

/// Units of `item` a row makes per 6000 ticks as its product.
fn product_units(row: &MoneyRow, item: &str) -> f64 {
    row.flows.iter().filter(|line| line.cause == "product" && line.item == item && line.per_6000 > 0.0).map(|line| line.per_6000).sum()
}

/// The object a row exists to make (its method's own product), when
/// that row is a priced recipe, chain or 100 % market line.
fn made_by(row: &MoneyRow) -> Option<String> {
    if !row.priced || !matches!(row.family, Family::Recipe | Family::Chain | Family::Market) {
        return None;
    }
    crate::guide_methods::own_product(&row.method)
}

/// Cost of one unit of `item` on `row` (`None`: the row does not make
/// it, makes none, or consumes nothing priced).
pub fn unit_cost(row: &MoneyRow, item: &str) -> Option<f64> {
    if made_by(row).as_deref() != Some(item) {
        return None;
    }
    let units = product_units(row, item);
    (units > 0.0 && row.cost_6000 > 0.0).then(|| row.cost_6000 / units)
}

/// Cheapest published cost of one unit of every object some row makes.
pub fn make_costs(rows: &[MoneyRow]) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for row in rows {
        let Some(item) = made_by(row) else { continue };
        let Some(cost) = unit_cost(row, &item) else { continue };
        out.entry(item).and_modify(|known| *known = known.min(cost)).or_insert(cost);
    }
    out
}

#[cfg(test)]
#[path = "guide_cost_tests.rs"]
mod tests;
