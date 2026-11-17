#![forbid(unsafe_code)]
//! The money guide as the ONE source of every coin figure the brain
//! decides with (plan rendements D12, D17, D22, R4, R7; decision
//! 2026-11-17 « Une seule source de profits : le guide money »). Every
//! reader reads the precomputed set (`guides_store::current()`, never a
//! build on demand) with the semantics of R7:
//! - `bot_d17`: the D17 price of a bot, the best profit per 6000 ticks
//!   among the priced, earning rows of ITS views (`brain_money::bot_rows`:
//!   each skill's band view, the activities its levels open) within its
//!   reach (`guide_feasible::feasible_for(..).within_reach()`), judged as
//!   `guide_read::best_profit_in` judges a bot (no stock, no wallet, the
//!   cycle's asks): a bot at one level in every skill reads exactly the
//!   D17 of `brain_train` (R7) at that level;
//! - `coin_rate`: what one tick of the bot's time is worth (the bid
//!   response, alchemy, derived demand): its D17 over 6000 ticks, never
//!   under `market_fit::MIN_COIN_RATE` (a tick is never free; no guide
//!   yet or no earning line: the floor);
//! - `method_profit_in`: a planner method's profit for the bot, its best
//!   priced row in the bot's rows (its own band);
//! - `kind_profits`: the best profit of each ambition kind, the guide's
//!   catalogue rows (priced, no condition impossible for every bot) under
//!   the kind of their band skill.
//!
//! The guide already prices what the market absorbs (D12: the current
//! price holds for the volume of its last five fills, then bids, NPC,
//! else 0): no reader applies a saturation of its own.

use lifescape_planner::guide_feasible::{feasible_for, BotState};
use lifescape_planner::guide_model::{MoneyRow, WINDOW_TICKS};
use lifescape_planner::market_fit::MIN_COIN_RATE;

use crate::guides_store::GuideSet;

/// What a D17 read knows of one bot (copied out of its live state).
pub struct BotKey<'a> {
    pub level_of: &'a dyn Fn(&str) -> u8,
    pub combat: u8,
    pub quests_done: &'a [String],
    pub ironman: bool,
}

/// The best earning row of the bot within its reach (module doc), `None`
/// when none earns.
pub fn best_row<'a>(set: &'a GuideSet, key: &BotKey) -> Option<&'a MoneyRow> {
    best_row_in(&crate::brain_money::bot_rows(set, key.level_of, &[]).rows, key)
}

/// `best_row` among `rows`, the bot's rows already read
/// (`brain_money::bot_rows` with no stock).
pub fn best_row_in<'a>(rows: &[&'a MoneyRow], key: &BotKey) -> Option<&'a MoneyRow> {
    let none = |_: &str| 0.0;
    let asks = |_: &str| None;
    let bot = BotState { level_of: key.level_of, combat: key.combat, quests_done: key.quests_done, held: &none, coins: None, ironman: key.ironman, asks: &asks };
    rows.iter()
        .copied()
        .filter(|row| row.priced && row.profit_6000 > 0.0 && feasible_for(*row, &bot).within_reach())
        .fold(None, |best: Option<&MoneyRow>, row| if best.is_none_or(|known| row.profit_6000 > known.profit_6000) { Some(row) } else { best })
}

/// The D17 price of the bot in `set` (module doc).
pub fn bot_d17(set: &GuideSet, key: &BotKey) -> Option<f64> {
    best_row(set, key).map(|row| row.profit_6000)
}

/// Coins per tick of a D17 price (module doc).
pub fn rate_of(d17: Option<f64>) -> f64 {
    d17.map(|profit| profit / WINDOW_TICKS).unwrap_or(0.0).max(MIN_COIN_RATE)
}

/// Coins per tick of the bot's time in the last published set (module
/// doc): the floor before the first set.
pub fn coin_rate(key: &BotKey) -> f64 {
    match crate::guides_store::current() {
        Some(set) => rate_of(bot_d17(&set, key)),
        None => MIN_COIN_RATE,
    }
}

/// Profit per 6000 ticks of a planner `method` in `rows` (the bot's
/// rows): its best priced row (the activity it works,
/// `activity::activity_of_method`), `None` when the guide has none.
pub fn method_profit_in(rows: &[&MoneyRow], method: &str) -> Option<f64> {
    let activity = lifescape_planner::activity::activity_of_method(method);
    rows.iter().filter(|row| row.priced && (row.method == activity || row.method == method)).map(|row| row.profit_6000).fold(None, |best: Option<f64>, profit| Some(best.map_or(profit, |known| known.max(profit))))
}

/// Best profit per ambition kind (module doc), one entry per kind, kinds
/// in name order.
pub fn kind_profits(set: &GuideSet) -> Vec<(String, f64)> {
    let mut out: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for row in set.money_catalog.rows.iter().filter(|row| row.priced && row.profit_6000.is_finite()) {
        if lifescape_planner::guide_feasible::conditions(&row.prereqs).iter().any(|condition| condition.impossible()) {
            continue;
        }
        let Some(skill) = lifescape_planner::activity::band_skill(&row.method, &row.style) else { continue };
        let Some(kind) = lifescape_planner::ambition_catalog::kind_for_skill(&skill) else { continue };
        let best = out.entry(kind.to_string()).or_insert(f64::MIN);
        *best = best.max(row.profit_6000);
    }
    out.into_iter().collect()
}

#[cfg(test)]
#[path = "brain_d17_tests.rs"]
mod tests;
