//! Market fit (user rule 2026-11-14: "supply must no longer crush
//! everything; bots play like real players"). Pure reads of the PUBLIC
//! book (resting bids and asks, 24 h volume) and of the bot's own life:
//! - `saturation_factor`: the share of a new batch the market absorbs
//!   (bids + a day of volume against resting asks + the batch). A money
//!   product a producer adds to a book counts at that share
//!   (`bid_response`).
//! - `sale_lot`: how much of a spare stack a real player puts up at once
//!   (what buyers take: bids + a day of volume, at least one inventory).
//! - `own_use_items`: what the bot's own open milestones will consume
//!   (logs for its firemaking climb, raw fish for its cooking, ores and
//!   bars for its smithing, bones for its prayer): kept, never sold.
//! - `MIN_COIN_RATE`: the floor of one tick of a bot's time (its rate is
//!   its D17 in the money guide, `brain_d17` on the orchestrator side).
//!
//! No other bot's state: the book is public (D22), the rest is the bot's.

use crate::ambition::{Ambition, MilestoneStatus, Target};

/// One inventory: the smallest lot a player bothers listing.
pub const SALE_LOT_MIN: i64 = 28;
/// Coins one tick of a bot's time is worth at least (never free time).
pub const MIN_COIN_RATE: f64 = 0.1;

/// Public book of one object as one bot reads it: other accounts' bids
/// (best first), every resting ask (cheapest first), units traded in the
/// last 24 h, the current price (median of the last five fills clamped
/// by the impact prices, `None` while it never traded) and the units
/// other producers publicly claimed (`market_claims`: an order taken on
/// the board, like a crafting order, 2026-11-14).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookView {
    pub bids: Vec<(i64, i64)>,
    pub asks: Vec<(i64, i64)>,
    pub volume_24h: i64,
    pub current: Option<i64>,
    pub claimed: i64,
}

impl BookView {
    pub fn best_bid(&self) -> Option<i64> {
        self.bids.iter().map(|(price, _)| *price).max()
    }

    pub fn best_ask(&self) -> Option<i64> {
        self.asks.iter().map(|(price, _)| *price).min()
    }

    pub fn bid_depth(&self) -> i64 {
        self.bids.iter().map(|(_, qty)| (*qty).max(0)).sum()
    }

    pub fn ask_depth(&self) -> i64 {
        self.asks.iter().map(|(_, qty)| (*qty).max(0)).sum()
    }

    /// Units resting on the ask side at or under `price`.
    pub fn asks_at_or_under(&self, price: i64) -> i64 {
        self.asks.iter().filter(|(ask, _)| *ask <= price).map(|(_, qty)| (*qty).max(0)).sum()
    }

    /// Coins to buy `units` from the cheapest asks up, `None` when the
    /// asks do not hold that many.
    pub fn buy_cost(&self, units: i64) -> Option<f64> {
        let mut asks = self.asks.clone();
        asks.sort_by_key(|(price, _)| *price);
        let mut left = units.max(0);
        let mut coins = 0.0;
        for (price, qty) in asks {
            if left == 0 {
                break;
            }
            let take = qty.max(0).min(left);
            coins += (take * price.max(1)) as f64;
            left -= take;
        }
        (left == 0).then_some(coins)
    }

    /// The asks left once other producers bought `units` of them,
    /// cheapest first (they take the best asks): their bid answers'
    /// purchases (`market_claims::purchases_by_others`, 2026-11-20).
    pub fn without_bought(&mut self, units: i64) {
        self.asks.sort_by_key(|(price, _)| *price);
        let mut left = units.max(0);
        for ask in &mut self.asks {
            if left == 0 {
                break;
            }
            let take = ask.1.max(0).min(left);
            ask.1 -= take;
            left -= take;
        }
        self.asks.retain(|(_, qty)| *qty > 0);
    }
}

/// Share in [0, 1] of a batch of `adding` units the market absorbs:
/// what buyers take (resting bid depth + 24 h volume) over what is
/// offered (resting asks + the batch). An object nobody offers is never
/// saturated (1.0, the first seller sets the price); no book at all is no
/// data, no decision (1.0).
pub fn saturation_factor(view: Option<&BookView>, adding: f64) -> f64 {
    let Some(view) = view else { return 1.0 };
    let offered = view.ask_depth();
    if offered <= 0 {
        return 1.0;
    }
    let absorbed = (view.bid_depth() + view.volume_24h.max(0)) as f64;
    (absorbed / (offered as f64 + adding.max(0.0))).clamp(0.0, 1.0)
}

/// Units of a spare stack a real player lists at once: what buyers take
/// now (bid depth + a day of volume), at least one inventory, never more
/// than the stack. No book: one inventory.
/// Days of traded volume a stock may hold before it is overstock.
pub const OVERSTOCK_DAYS: i64 = 2;

/// True when `stock` free units exceed what the market absorbs: the bids
/// plus `OVERSTOCK_DAYS` of volume, never under one inventory (user
/// 2026-11-18: nathan_t's 564 mithril full helms, one trade in 30 h).
pub fn overstocked(stock: i64, view: &BookView) -> bool {
    stock > SALE_LOT_MIN.max(view.bid_depth() + OVERSTOCK_DAYS * view.volume_24h.max(0))
}

/// Actions of a stock lot whose product the market absorbs (point 2,
/// user 2026-11-18: never transform a stock into overstock): the bids
/// plus `OVERSTOCK_DAYS` of volume, less the units resting offered and
/// the `held` units the bot already has, over `per_action` units made.
pub fn absorbable_actions(view: &BookView, held: i64, per_action: i64) -> i64 {
    let room = view.bid_depth() + OVERSTOCK_DAYS * view.volume_24h.max(0) - view.ask_depth() - held.max(0);
    (room.max(0)) / per_action.max(1)
}

pub fn sale_lot(spare: i64, view: Option<&BookView>) -> i64 {
    let absorbed = view.map(|book| book.bid_depth() + book.volume_24h.max(0)).unwrap_or(0);
    absorbed.max(SALE_LOT_MIN).min(spare.max(0))
}

/// Objects the bot's own open milestones consume (debug names, sorted):
/// every tranche of an open level milestone (from its live progress to
/// its target) burns its method's inputs (`sizing::tranche_consumption`:
/// logs for firemaking, raw fish for cooking, ores for smelting, bones
/// for prayer, runes for magic), and an open production quota consumes
/// its recipe's inputs and theirs (bars, then ores). Gathered products
/// the bot will use itself are kept, never sold (a firemaker chops its
/// logs to burn them: live 2026-11-13, b8jxrdd2 sold 2035 willow logs
/// its abandoned firemaking chain had cut).
pub fn own_use_items(ambition: &Ambition) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for stone in ambition.milestones.iter().filter(|stone| stone.status != MilestoneStatus::Done) {
        match &stone.target {
            Target::Level { skill, level } => {
                let from = stone.progress.0.clamp(1, 99) as u8;
                let mut start = from;
                for cut in crate::tranches::tranches(skill, from, *level) {
                    let method = crate::tranches::reach_method(skill, start);
                    for (item, _) in crate::sizing::tranche_consumption(&method, start, cut) {
                        // A smith smelts its own bars (2026-11-14): the ores
                        // its anvil tranches will melt stay too.
                        if skill == "smithing" {
                            for leg in crate::obtain_legs::production_legs(&item).into_iter().filter(|leg| leg.method.starts_with("smelt:")) {
                                out.extend(leg.inputs.iter().map(|(name, _)| name.clone()));
                            }
                        }
                        out.push(item);
                    }
                    start = cut;
                }
                // A magic climb past 21 alchs: its nature and fire runes.
                out.extend(crate::training::kept_inputs(skill, *level));
            }
            Target::Produce { item, .. } => {
                for leg in crate::obtain_legs::production_legs(item) {
                    for (input, _) in &leg.inputs {
                        out.push(input.clone());
                        for deeper in crate::obtain_legs::production_legs(input) {
                            out.extend(deeper.inputs.iter().map(|(name, _)| name.clone()));
                        }
                    }
                }
            }
            Target::Equip { .. } | Target::Coins { .. } => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
#[path = "market_fit_tests.rs"]
mod tests;
