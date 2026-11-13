//! Production profit at market prices (2026-11-13): the hook that turns
//! buy orders into production. A resting bid lifts the price a producer
//! can get; `chain_profit` says whether making the object pays at that
//! price, from the game's recipes (`gamedata::obtain`), market prices
//! and the measured yield (rates).
//!
//! For the ambition draw and producers (agent D, `market_demand`):
//! ```text
//! let prices = vec![("iron_scimitar".into(), bid_or_reference), ("iron_bar".into(), bar_reference)];
//! let verdict = chain_profit("iron_scimitar", &prices, measured_units_per_6000)?;
//! if verdict.profit_per_6000 > 0.0 { /* worth making: train/produce, buy inputs */ }
//! ```
//! - `prices`: per-unit market prices by debug name — the object's own
//!   (the best resting bid when a buyer waits, else the reference: 24 h
//!   VWAP, last print, mid, 1 coin) and every input's (the reference, or
//!   the best ask when the producer must buy it). Never an NPC shop price.
//! - `units_per_6000`: measured output per 6000 ticks (rates page,
//!   `task_yields`), or the cold-start prior.
//! - Inputs a way consumes are costed at market even when the producer
//!   gathers them itself (the opportunity cost: it could sell them).
//!   Tools are not consumed and cost nothing here.
//!
//! No floor, no shop anchor: a negative margin is a real answer (do not
//! make it at these prices). Pure: no market, no rates read here.

use lifescape_gamedata::obtain::ways;

/// Profit of making one object by its best production way.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainProfit {
    pub item: String,
    /// The way chosen (`smith`, `cook`, `gather` …: the obtain detail).
    pub way: String,
    pub sale_price: f64,
    /// Inputs per unit made: `(input, quantity, unit price)`.
    pub inputs: Vec<(String, f64, f64)>,
    /// Inputs plus coin fees per unit made.
    pub input_cost: f64,
    /// `sale_price - input_cost`.
    pub margin: f64,
    pub units_per_6000: f64,
    /// `margin × units_per_6000`.
    pub profit_per_6000: f64,
}

/// Why no profit could be computed (typed, never a silent zero).
#[derive(Debug, Clone, PartialEq)]
pub enum ProfitError {
    /// No production way makes this object in F2P (merchant or drop only).
    NotProduced(String),
    /// The object's own price is missing from `prices`.
    MissingSalePrice(String),
    /// Every production way needs an input with no price.
    MissingInputPrice { item: String, input: String },
    /// Yield must be a finite number ≥ 0.
    BadYield(String),
}

impl std::fmt::Display for ProfitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfitError::NotProduced(item) => write!(formatter, "no production way makes {item}"),
            ProfitError::MissingSalePrice(item) => write!(formatter, "no price for {item}"),
            ProfitError::MissingInputPrice { item, input } => write!(formatter, "no price for input {input} of {item}"),
            ProfitError::BadYield(text) => write!(formatter, "bad yield {text}"),
        }
    }
}

fn price_of(prices: &[(String, f64)], name: &str) -> Option<f64> {
    prices.iter().find(|(known, _)| known == name).map(|(_, price)| *price).filter(|price| price.is_finite() && *price >= 0.0)
}

/// Profit of making `item` at `prices` with the measured yield: the
/// production way with the best margin among those whose inputs are all
/// priced (`gamedata::obtain` recipes; an action yielding several units
/// spreads its inputs over them).
pub fn chain_profit(item: &str, prices: &[(String, f64)], units_per_6000: f64) -> Result<ChainProfit, ProfitError> {
    if !units_per_6000.is_finite() || units_per_6000 < 0.0 {
        return Err(ProfitError::BadYield(format!("{units_per_6000}")));
    }
    let sale_price = price_of(prices, item).ok_or_else(|| ProfitError::MissingSalePrice(item.to_string()))?;
    let mut best: Option<ChainProfit> = None;
    let mut missing: Option<String> = None;
    let mut produced = false;
    for way in ways(item).into_iter().filter(|way| way.is_production()) {
        produced = true;
        let out = way.qty.map(|(low, high)| (low + high) as f64 / 2.0).filter(|qty| *qty > 0.0).unwrap_or(1.0);
        let mut inputs = Vec::new();
        let mut cost = way.coins.max(0) as f64 / out;
        let mut priced = true;
        for (input, qty) in &way.inputs {
            let Some(unit) = price_of(prices, input) else {
                missing.get_or_insert_with(|| input.clone());
                priced = false;
                break;
            };
            let per_unit = *qty as f64 / out;
            cost += per_unit * unit;
            inputs.push((input.clone(), per_unit, unit));
        }
        if !priced {
            continue;
        }
        let margin = sale_price - cost;
        if best.as_ref().is_none_or(|known| margin > known.margin) {
            let label = way.detail.split_whitespace().next().unwrap_or("make").to_string();
            best = Some(ChainProfit {
                item: item.to_string(),
                way: label,
                sale_price,
                inputs,
                input_cost: cost,
                margin,
                units_per_6000,
                profit_per_6000: margin * units_per_6000,
            });
        }
    }
    match (best, produced, missing) {
        (Some(found), _, _) => Ok(found),
        (None, false, _) => Err(ProfitError::NotProduced(item.to_string())),
        (None, true, Some(input)) => Err(ProfitError::MissingInputPrice { item: item.to_string(), input }),
        (None, true, None) => Err(ProfitError::NotProduced(item.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{chain_profit, ProfitError};

    fn prices(rows: &[(&str, f64)]) -> Vec<(String, f64)> {
        rows.iter().map(|(name, price)| (name.to_string(), *price)).collect()
    }

    /// A smithed piece costs its bars at market: at the 1-coin bootstrap
    /// nothing pays, a bid above the bars makes it worth making.
    #[test]
    fn smithing_pays_once_the_bid_covers_the_bars() {
        let cheap = chain_profit("iron_scimitar", &prices(&[("iron_scimitar", 1.0), ("iron_bar", 15.0)]), 100.0).expect("smithable");
        assert!(cheap.inputs.iter().any(|(input, qty, _)| input == "iron_bar" && *qty >= 1.0), "{cheap:?}");
        assert!(cheap.margin < 0.0, "1 coin never pays the bars: {cheap:?}");
        let bid = cheap.input_cost + 10.0;
        let paid = chain_profit("iron_scimitar", &prices(&[("iron_scimitar", bid), ("iron_bar", 15.0)]), 100.0).expect("smithable");
        assert!((paid.margin - 10.0).abs() < 1e-9 && (paid.profit_per_6000 - 1000.0).abs() < 1e-6, "{paid:?}");
    }

    /// Gathered objects consume nothing: the sale price is the margin.
    #[test]
    fn gathering_margin_is_the_price() {
        let logs = chain_profit("logs", &prices(&[("logs", 3.0)]), 400.0).expect("gatherable");
        assert_eq!(logs.input_cost, 0.0);
        assert_eq!(logs.profit_per_6000, 1200.0);
    }

    #[test]
    fn missing_numbers_are_typed() {
        assert_eq!(chain_profit("iron_scimitar", &prices(&[("iron_bar", 3.0)]), 10.0), Err(ProfitError::MissingSalePrice("iron_scimitar".to_string())));
        assert!(matches!(chain_profit("iron_scimitar", &prices(&[("iron_scimitar", 30.0)]), 10.0), Err(ProfitError::MissingInputPrice { .. })));
        assert!(matches!(chain_profit("iron_scimitar", &prices(&[("iron_scimitar", 30.0)]), f64::NAN), Err(ProfitError::BadYield(_))));
        assert!(matches!(chain_profit("shears", &prices(&[("shears", 1.0)]), 10.0), Err(ProfitError::NotProduced(_))));
    }
}
