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
//!   against an autonomous one by `buy_weight` (2026-11-17, user lever
//!   « entraînement marché plus fréquent »): a continuous curve of the
//!   trait, ×2^(2 − 4 × affinity) on the line's cost (×1 at 0.5, ×0.44 at
//!   0.8, ×3 at 0.1, ×4 at 0, ×0.25 at 1): a market bot (≥ 0.6) buys its
//!   inputs (bars, logs, raw fish, runes, bones) unless their coins, at
//!   its D17 price, make buying far slower than working them up itself;
//!   an autarkic one farms unless buying is far faster (a rune pickaxe
//!   nobody can smith: every bot buys). The money guide's market lines
//!   keep the linear `market_weight` (1.5 − affinity, not this lever);
//! - a negative score (the line earns more than its time costs) keeps the
//!   same order: the weight divides it instead of multiplying.
//!
//! Lower score wins. An XP-less line has none; an Ironman never scores a
//! market line. The choice among the scored lines is the draw of R7
//! (`pick`, `guide_pick`: the lines the bot works now first, then those
//! it works after a tool or a lot's coins; within that class a weighted
//! draw among the lines within 15 % of the best score, D10, and the
//! unmeasured lines' 5 % of exploration, D14); `best` is the head of the
//! order (the score alone, what the R6 examples state).

use crate::guide_model::{Supply, TrainingRow, WINDOW_TICKS};
use crate::model::Profile;

/// Weight of the coins (converted to ticks) against the time (D13).
pub fn coin_weight(thrift: f64) -> f64 {
    0.5 + thrift.clamp(0.0, 1.0)
}

/// Weight of a market line's cost (D15, `cost::choose_market_or_farm`,
/// the money guide's market lines `guide_pick::merit_money`).
pub fn market_weight(affinity: f64) -> f64 {
    1.5 - affinity.clamp(0.0, 1.0)
}

/// Weight of a MARKET training line's cost for a bot of `affinity`
/// (module doc): 2^(2 − 4 × affinity), continuous and decreasing, 1 at
/// the neutral 0.5.
pub fn buy_weight(affinity: f64) -> f64 {
    2f64.powf(2.0 - 4.0 * affinity.clamp(0.0, 1.0))
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

/// The score of `row` for `profile` (module doc); `None` for a line
/// without XP or a market line for an Ironman. Whether the bot can run it
/// is judged per bot by `rank_rows` (D22, `guide_feasible`).
/// Without an earning money line (`ticks_per_coin` `None`) the coins
/// convert to nothing: time alone ranks.
pub fn score_row(row: &TrainingRow, profile: &Profile, ticks_per_coin: Option<f64>) -> Option<LineScore> {
    if row.xp_6000 <= 0.0 {
        return None;
    }
    if profile.ironman && row.supply == Supply::Market {
        return None;
    }
    let own = WINDOW_TICKS / row.xp_6000;
    let coins = ticks_per_coin.map(|per_coin| row.coins_per_xp * per_coin).unwrap_or(0.0);
    let total = own + coin_weight(profile.thrift) * coins;
    let score = if row.supply == Supply::Market {
        let weight = buy_weight(profile.market_affinity);
        if total >= 0.0 { total * weight } else { total / weight }
    } else {
        total
    };
    Some(LineScore { own_ticks_per_xp: own, coin_ticks_per_xp: coins, score })
}

/// The scored lines of `rows` for `profile`, best (lowest) first; ties
/// keep the guide's order.
/// `within_reach`: the bot's judgement of a line (D22: feasible, or once a
/// tool or a lot's coins are obtained); lines out of reach never rank.
pub fn rank_rows<'a>(rows: &'a [TrainingRow], profile: &Profile, ticks_per_coin: Option<f64>, within_reach: &dyn Fn(&TrainingRow) -> bool) -> Vec<(LineScore, &'a TrainingRow)> {
    let mut out: Vec<(LineScore, &TrainingRow)> = rows.iter().filter(|row| within_reach(row)).filter_map(|row| score_row(row, profile, ticks_per_coin).map(|score| (score, row))).collect();
    out.sort_by(|left, right| left.0.score.total_cmp(&right.0.score));
    out
}

/// The best scored line of `ranked` (`rank_rows`): the head of the
/// order, what the score alone says (the draw is `pick`).
pub fn best<'a>(ranked: &[(LineScore, &'a TrainingRow)]) -> Option<&'a TrainingRow> {
    ranked.first().map(|(_, row)| *row)
}

/// One scored line of a bot with its class (`guide_pick::class_order`:
/// 0 feasible now, 1 after a lot's coins, 2 after a tool to obtain).
#[derive(Debug, Clone)]
pub struct Classed<'a> {
    pub score: LineScore,
    pub row: &'a TrainingRow,
    pub class: u8,
}

/// The scored lines of `rows` for `profile` with the class `class_of`
/// gives each (`None`: out of the bot's reach, never drawn), best score
/// first.
pub fn rank_classed<'a>(rows: &'a [TrainingRow], profile: &Profile, ticks_per_coin: Option<f64>, class_of: &dyn Fn(&TrainingRow) -> Option<u8>) -> Vec<Classed<'a>> {
    let mut out: Vec<Classed> = rows.iter().filter_map(|row| Some(Classed { class: class_of(row)?, score: score_row(row, profile, ticks_per_coin)?, row })).collect();
    out.sort_by(|left, right| left.score.score.total_cmp(&right.score.score));
    out
}

/// The line a bot trains with (R7): the draw of `guide_pick` over the
/// first non-empty class of `ranked`, its score the line's (minus its
/// weighted ticks per XP), its weight `merit_training`, its runs the
/// yield's measured runs; with the index of the line in `ranked`.
pub fn pick(ranked: &[Classed], seed: u64) -> Option<(usize, crate::guide_pick::Drawn)> {
    let classes: Vec<u8> = ranked.iter().map(|line| line.class).collect();
    let (_, members) = crate::guide_pick::first_class(&classes);
    let candidates: Vec<crate::guide_pick::Candidate> = members
        .iter()
        .map(|index| {
            let line = &ranked[*index];
            crate::guide_pick::Candidate { score: -line.score.score, weight: crate::guide_pick::merit_training(line.score.score), runs: line.row.confidence.runs }
        })
        .collect();
    let drawn = crate::guide_pick::draw(&candidates, seed)?;
    Some((members[drawn.index], drawn))
}

#[cfg(test)]
#[path = "train_score_tests.rs"]
mod tests;
