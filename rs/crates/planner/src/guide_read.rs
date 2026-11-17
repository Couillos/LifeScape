//! Read side of the guides (plan rendements R4, R7): a VIEW is one
//! guide built for a bot at one level (or the catalogue, each method at
//! its gate) with no quest done and nothing held, every row of every
//! activity in activity order, before the reader's filter. A reader's
//! filter is applied here by reading the view, never by rebuilding it:
//! - `skill`, `family`: rows left out (money: the activity's band skill;
//!   training: the row's skill), order kept;
//! - `level` (a level of the view's band, the view built at the band's
//!   LAST level): activities gated above it left out, accessibility at
//!   it, ticks to the band's end from it; the rates stay the band's last
//!   level's (exact at 5, 10, ..., 95, 99);
//! - `quests`: accessibility with them (quest prerequisites, quest
//!   points), done quest shortcuts left out, and the training D17 price
//!   re-read from the money view with them (`d17` of `finish_training`);
//!   a chain line keeps the farm-or-buy choices of the bot without
//!   quests (its own D17 outside the chains);
//! - `held`: the money recipe rows re-evaluated on the bot's stock by the
//!   caller (`guide_money::recipe_row`, `replaced`), every other row
//!   kept.
//!
//! Then usable rows first (not blocked, prerequisites met), then by
//! value, ties in activity order: the order `money_guide` and
//! `training_guide` give, which are `finish` of a fresh view.

use std::collections::HashMap;

use crate::guide_model::{GuideFilter, MoneyRow, RateAnswer, RateAsk, TrainingRow, Unrated, WINDOW_TICKS};

/// One activity nothing estimates, with its style (the skill filter).
#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    pub method: String,
    pub style: String,
    pub reason: String,
}

/// A recipe row's place (its ask and yield): re-evaluated on a bot's
/// stock (`held`) without rebuilding the view.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipePlace {
    /// Index of the row in `MoneyView::rows`.
    pub row: usize,
    pub ask: RateAsk,
    pub answer: RateAnswer,
}

/// Money rows of a bot at `level` (`None`: the catalogue), no quest
/// done, nothing held, activity order, `accessible` unset.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MoneyView {
    pub level: Option<u8>,
    pub rows: Vec<MoneyRow>,
    pub gaps: Vec<Gap>,
    pub recipes: Vec<RecipePlace>,
}

/// Training rows of a bot at `level` (`None`: the catalogue), every
/// skill, no quest done, activity order, then the quest shortcuts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrainingView {
    pub level: Option<u8>,
    pub rows: Vec<TrainingRow>,
    pub gaps: Vec<Gap>,
}

fn gated_out(method: &str, filter: &GuideFilter) -> bool {
    filter.level.is_some_and(|level| crate::guide_methods::gate_level(method) > level)
}

fn money_skill_out(method: &str, style: &str, filter: &GuideFilter) -> bool {
    filter.skill.as_ref().is_some_and(|skill| crate::activity::band_skill(method, style).as_deref() != Some(skill.as_str()))
}

/// Usable first (not blocked, prerequisites met), then by `value`
/// (stable: ties keep the activity order).
fn usable_first<T>(rows: &mut [T], usable: impl Fn(&T) -> bool, value: impl Fn(&T) -> f64) {
    rows.sort_by(|left, right| usable(left).cmp(&usable(right)).reverse().then(value(right).total_cmp(&value(left))));
}

/// First gap per method, in activity order.
fn unrated_of<'a>(gaps: impl Iterator<Item = &'a Gap>) -> Vec<Unrated> {
    let mut out: Vec<Unrated> = Vec::new();
    for gap in gaps {
        if !out.iter().any(|known| known.method == gap.method) {
            out.push(Unrated { method: gap.method.clone(), reason: gap.reason.clone() });
        }
    }
    out
}

/// The money guide of `filter` read from `view`; `replaced` holds the
/// rows re-evaluated for the reader (a recipe on its stock) by index.
pub fn finish_money(view: &MoneyView, filter: &GuideFilter, replaced: &HashMap<usize, MoneyRow>) -> (Vec<MoneyRow>, Vec<Unrated>) {
    let mut rows: Vec<MoneyRow> = Vec::new();
    for (index, stored) in view.rows.iter().enumerate() {
        if gated_out(&stored.method, filter) || money_skill_out(&stored.method, &stored.style, filter) || filter.family.as_deref().is_some_and(|family| family != stored.family.as_str()) {
            continue;
        }
        let mut row = replaced.get(&index).unwrap_or(stored).clone();
        row.accessible = row.blocked.is_empty() && crate::guide_prereq::passes(&row.prereqs, filter);
        rows.push(row);
    }
    usable_first(&mut rows, |row| row.blocked.is_empty() && row.accessible, |row| row.profit_6000);
    let unrated = unrated_of(view.gaps.iter().filter(|gap| !gated_out(&gap.method, filter) && !money_skill_out(&gap.method, &gap.style, filter)));
    (rows, unrated)
}

/// Profit per 6000 ticks of the best accessible row of `rows` (D17),
/// `None` when none earns.
pub fn best_profit_of(rows: &[MoneyRow]) -> Option<f64> {
    rows.iter().filter(|row| row.accessible && row.profit_6000 > 0.0).map(|row| row.profit_6000).fold(None, |best: Option<f64>, profit| Some(best.map_or(profit, |known| known.max(profit))))
}

/// D17 of a bot of `filter` read in `view` without copying a row:
/// `best_profit_of` of `finish_money(view, filter)` (its skill and family
/// ignored: a bot's best line is any line).
pub fn best_profit_in(view: &MoneyView, filter: &GuideFilter) -> Option<f64> {
    view.rows
        .iter()
        .filter(|row| !gated_out(&row.method, filter) && row.blocked.is_empty() && row.profit_6000 > 0.0 && crate::guide_prereq::passes(&row.prereqs, filter))
        .map(|row| row.profit_6000)
        .fold(None, |best: Option<f64>, profit| Some(best.map_or(profit, |known| known.max(profit))))
}

/// Level a training row was read at: the view's, else its method's gate.
fn row_level_of(view: &TrainingView, method: &str) -> u8 {
    view.level.unwrap_or_else(|| crate::guide_methods::gate_level(method)).clamp(1, 99)
}

/// The D17 conversion of one training row for a bot whose best money
/// line earns `profit` per 6000 ticks, at `level` (`guide_training`).
pub fn convert(row: &mut TrainingRow, profit: Option<f64>, level: u8) {
    let spent = (-row.coins_6000).max(0.0);
    row.converted_ticks_6000 = match profit {
        Some(profit) if profit > 0.0 && spent > 0.0 => spent / (profit / WINDOW_TICKS),
        _ => 0.0,
    };
    row.effective_xp_6000 = if row.blocked.is_empty() { row.xp_6000 * WINDOW_TICKS / (WINDOW_TICKS + row.converted_ticks_6000) } else { 0.0 };
    let effective = row.effective_xp_6000;
    row.ticks_to_band_end = (effective > 0.0).then(|| crate::guide_training::xp_to_band_end(level) / (effective / WINDOW_TICKS));
}

/// The training guide of `filter` read from `view`. `d17`: the D17
/// price of a bot of `filter` at a band (`band_of`'s first level), given
/// when it differs from the view's own (quests, a level under the
/// view's): every method row is then converted again with it.
pub fn finish_training(view: &TrainingView, filter: &GuideFilter, d17: Option<&dyn Fn(i64) -> Option<f64>>) -> (Vec<TrainingRow>, Vec<Unrated>) {
    let mut rows: Vec<TrainingRow> = Vec::new();
    for stored in &view.rows {
        let quest = stored.method.strip_prefix("quest:");
        if filter.skill.as_ref().is_some_and(|skill| *skill != stored.skill) || quest.is_some_and(|key| filter.quests_done.iter().any(|done| done == key)) {
            continue;
        }
        if quest.is_none() && gated_out(&stored.method, filter) {
            continue;
        }
        let mut row = stored.clone();
        if let (None, Some(d17)) = (quest, d17) {
            // `guide_training::row_of`: the bot's level, else the gate.
            let level = filter.level.unwrap_or_else(|| row_level_of(view, &row.method));
            let profit = d17(row.band);
            convert(&mut row, profit, level);
        }
        row.accessible = row.blocked.is_empty() && crate::guide_prereq::passes(&row.prereqs, filter);
        rows.push(row);
    }
    usable_first(&mut rows, |row| row.blocked.is_empty() && row.accessible, |row| row.effective_xp_6000);
    let unrated = unrated_of(view.gaps.iter().filter(|gap| !gated_out(&gap.method, filter)));
    (rows, unrated)
}

#[cfg(test)]
#[path = "guide_read_tests.rs"]
mod tests;
