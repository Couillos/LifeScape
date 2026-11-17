//! Score of a training-guide line for one bot (plan rendements R6, D13,
//! D17): the public line (`guide_model::TrainingRow`, precomputed by the
//! guides store) weighed by the bot's two profile traits.
//!
//! - time per XP = the line's own ticks per XP (6000 / XP per 6000: the
//!   work, its trips, its waits; an autonomous line its farming too);
//! - coins per XP (spent > 0, earned < 0) converted to ticks at the bot's
//!   best accessible money line (D17: 6000 / profit per 6000 ticks per
//!   coin);
//! - thrift (rapide 0 ↔ économe 1, D13) weighs the coins against the time:
//!   `coin_weight = 0.5 + thrift` (0.5 for a fast bot that spends to go
//!   faster, 1.5 for a thrifty one; 1 = the neutral D17 total);
//! - market_affinity (autarcique 0 ↔ marché 1) weighs a MARKET line
//!   against an autonomous one as `cost::choose_market_or_farm` does
//!   (×(1.5 − affinity) on its cost): a market bot buys its yew logs as
//!   soon as buying is faster, an autarkic one cuts them unless buying is
//!   far faster (a rune pickaxe nobody can smith: every bot buys);
//! - a negative score (the line earns more than its time costs) keeps the
//!   same order: the weight divides it instead of multiplying.
//!
//! Lower score wins. A blocked, inaccessible or XP-less line has none; an
//! Ironman never scores a market line. The choice among the scored lines
//! is `choose`: today the best score; R7 replaces it with the weighted
//! draw D10 (lines within 15 % of the top, D14 exploration).

use crate::guide_model::{Supply, TrainingRow, WINDOW_TICKS};
use crate::model::Profile;

/// Weight of the coins (converted to ticks) against the time (D13).
pub fn coin_weight(thrift: f64) -> f64 {
    0.5 + thrift.clamp(0.0, 1.0)
}

/// Weight of a market line's cost (D15, `cost::choose_market_or_farm`).
pub fn market_weight(affinity: f64) -> f64 {
    1.5 - affinity.clamp(0.0, 1.0)
}

/// One line's terms for a bot, per XP of the line's skill.
#[derive(Debug, Clone, PartialEq)]
pub struct LineScore {
    pub own_ticks_per_xp: f64,
    /// Coins per XP converted at D17 (negative: the line earns).
    pub coin_ticks_per_xp: f64,
    /// The weighted total (lower wins).
    pub score: f64,
}

/// Ticks one coin costs a bot whose best accessible money line earns
/// `best_profit` per 6000 ticks (D17); `None` when no line earns.
pub fn ticks_per_coin(best_profit: Option<f64>) -> Option<f64> {
    best_profit.filter(|profit| *profit > 0.0).map(|profit| WINDOW_TICKS / profit)
}

/// The score of `row` for `profile` (module doc); `None` for a line the
/// bot cannot run (blocked, inaccessible, no XP, market for an Ironman).
/// Without an earning money line (`ticks_per_coin` `None`) the coins
/// convert to nothing: time alone ranks.
pub fn score_row(row: &TrainingRow, profile: &Profile, ticks_per_coin: Option<f64>) -> Option<LineScore> {
    if !row.blocked.is_empty() || !row.accessible || row.xp_6000 <= 0.0 {
        return None;
    }
    if profile.ironman && row.supply == Supply::Market {
        return None;
    }
    let own = WINDOW_TICKS / row.xp_6000;
    let coins = ticks_per_coin.map(|per_coin| row.coins_per_xp * per_coin).unwrap_or(0.0);
    let total = own + coin_weight(profile.thrift) * coins;
    let score = if row.supply == Supply::Market {
        let weight = market_weight(profile.market_affinity);
        if total >= 0.0 { total * weight } else { total / weight }
    } else {
        total
    };
    Some(LineScore { own_ticks_per_xp: own, coin_ticks_per_xp: coins, score })
}

/// The scored lines of `rows` for `profile`, best (lowest) first; ties
/// keep the guide's order.
pub fn rank_rows<'a>(rows: &'a [TrainingRow], profile: &Profile, ticks_per_coin: Option<f64>) -> Vec<(LineScore, &'a TrainingRow)> {
    let mut out: Vec<(LineScore, &TrainingRow)> = rows.iter().filter_map(|row| score_row(row, profile, ticks_per_coin).map(|score| (score, row))).collect();
    out.sort_by(|left, right| left.0.score.total_cmp(&right.0.score));
    out
}

/// The line a bot trains with among `ranked` (`rank_rows`): the best
/// score. R7 extension point: the weighted draw (D10, D14) takes this
/// place, fed the same ranked lines and the bot's seed.
pub fn choose<'a>(ranked: &[(LineScore, &'a TrainingRow)]) -> Option<&'a TrainingRow> {
    ranked.first().map(|(_, row)| *row)
}

#[cfg(test)]
#[path = "train_score_tests.rs"]
mod tests;
