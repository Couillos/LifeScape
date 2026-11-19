//! Shared demand at a bot's pick (plan équilibre step 2, user
//! 2026-11-19): the money guide is built every `REFRESH_PERIOD` with the
//! order board of its build (every claim); a bot choosing NOW reads the
//! claims of the OTHER producers as they stand (`orchestrator::
//! market_claims`, public: units per object, never who) and reprices the
//! products of a row on them, so two bots choosing one after the other
//! never both get the whole demand of an object.
//!
//! A row's product flows (positive, not coins) are walked again down the
//! object's SHARED ladder (`guide_engine::sale_ladder`: bids and the
//! current price's volume, minus the asks resting at or under the price
//! and the claims of the others, then the shop); the row's profit moves
//! by the difference with the guide's value of those flows. Inputs keep
//! the guide's price (a purchase is not a sale shared with producers).
//! Pure: the caller hands the quotes (the cycle's book, the live claims)
//! and the shops' prices.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::guide_engine::{ladder_value, sale_ladder, SaleLevel};
use crate::guide_model::{MoneyRow, Quote, ShopNeed};

/// A bot's reading of the shared demand: quotes with the claims of the
/// other producers, the best F2P shop of an object, one ladder per
/// object built once.
pub struct SharedDemand<'a> {
    quote: &'a dyn Fn(&str) -> Option<Quote>,
    shop: &'a dyn Fn(&str) -> Option<(f64, ShopNeed)>,
    ladders: RefCell<HashMap<String, Option<std::rc::Rc<Vec<SaleLevel>>>>>,
}

impl<'a> SharedDemand<'a> {
    pub fn new(quote: &'a dyn Fn(&str) -> Option<Quote>, shop: &'a dyn Fn(&str) -> Option<(f64, ShopNeed)>) -> Self {
        SharedDemand { quote, shop, ladders: RefCell::new(HashMap::new()) }
    }

    /// The shared sale ladder of `item`, `None` for an object without a
    /// quote (the guide's value then stands).
    pub fn ladder(&self, item: &str) -> Option<std::rc::Rc<Vec<SaleLevel>>> {
        if let Some(known) = self.ladders.borrow().get(item) {
            return known.clone();
        }
        let built = (self.quote)(item).map(|quote| std::rc::Rc::new(sale_ladder(&quote, (self.shop)(item))));
        self.ladders.borrow_mut().insert(item.to_string(), built.clone());
        built
    }

    /// The profit per 6000 ticks of `row` for this bot: each product
    /// flow valued down its shared ladder instead of the guide's.
    pub fn profit(&self, row: &MoneyRow) -> f64 {
        let mut profit = row.profit_6000;
        for line in row.flows.iter().filter(|line| line.per_6000 > 0.0 && line.item != "coins") {
            let Some(ladder) = self.ladder(&line.item) else { continue };
            profit += ladder_value(&ladder, line.per_6000) - line.value_6000;
        }
        profit
    }
}

#[cfg(test)]
#[path = "guide_share_tests.rs"]
mod tests;
