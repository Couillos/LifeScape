//! Training as an investment (plan progression A1, user 2026-11-21): ONE
//! common value, « gain per 6000 ticks of an objective, the climb
//! included », comparable between a fast line that needs no level and a
//! long climb that unlocks a lucrative production (crafting 70 -> amulet
//! of power, smithing 85 + mining 85 -> rune equipment, magic 55 ->
//! alchemy).
//!
//! For one objective P (an object sold or wanted at `unit_price`, or a
//! caster's alchemy) and a bot at levels `L0`:
//!
//! ```text
//! gates      = (skill, level) the way of P needs, plus the gates of every
//!              input NOTHING prices (the bot has to make it itself)
//! climb      = Σ over gates above L0 of  xp_between(L0, gate)
//!                                        / best_xp_rate(skill, L0..gate)      [ticks]
//! gain_tick  = (unit_price × made − Σ inputs bought at market) per tick       [coins]
//!              (the way's flows per 6000 ticks, `work_rate`, priced like
//!               `profit::chain_profit`; units made per tick from the rate)
//! total      = min( gain_tick × (H − climb) , Σ offers (price − cost) × units )  [coins]
//!              (each buyer pays HIS price: `ladder_value`, never the best for all)
//! value_6000 = total / (H / 6000)                    (0 when climb ≥ H)
//! ```
//!
//! - `H` = `INVESTMENT_HORIZON_TICKS` (720 000 ticks = 120 production
//!   hours at 600 ms, 12 h at the 60 ms dev tick): the span over which a
//!   level pays back. Long enough for a 100 000-tick climb to count for a
//!   seventh of it, short enough that an unreachable level (climb ≥ H)
//!   is worth nothing. The climb earns nothing in this model (its own
//!   products sell on top, an upside the figure ignores).
//! - `capacity` = units the market wants (the unmet bids' quantity, the
//!   units resting on the asks for alchemy): a one-unit bid never weighs
//!   what a standing demand weighs.
//! - `best_xp_rate` = the best XP per tick of the skill among the
//!   catalogue's methods the level opens (`guide_methods::catalogue`,
//!   rates derived from the dump's units, `work_rate`): never measured
//!   here, never a hand constant.
//!
//! Used by A4 (`ambition_value`: the weight of an unserved demand) and by
//! the training line choice (`pull_per_tick`: the time value of a climb
//! that opens a lucrative way). Pure: prices and levels come in as
//! arguments.

use crate::work_rate::{RateReader, WorkRate, WINDOW_TICKS};

/// Ticks over which a level pays back (module doc).
pub const INVESTMENT_HORIZON_TICKS: f64 = 720_000.0;
/// Deepest chain of unpriced inputs followed for gates.
const GATE_DEPTH: u8 = 3;

/// One level a way needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub skill: String,
    pub level: u8,
    /// Ticks to climb it from the bot's level (0 when reached).
    pub climb_ticks: f64,
}

/// The investment of one objective for one bot.
#[derive(Debug, Clone, PartialEq)]
pub struct Investment {
    pub item: String,
    pub method: String,
    pub gates: Vec<Gate>,
    /// Σ of the gates' climb ticks.
    pub climb_ticks: f64,
    /// Coins gained per tick once every gate is reached.
    pub gain_per_tick: f64,
    /// Margin of one unit (price minus inputs), for the capacity cap.
    pub margin: f64,
    pub capacity: f64,
    /// Units one bot makes per tick (what a holder covers of the demand).
    pub units_per_tick: f64,
    /// The offers served, best price first: (unit price, units). Each
    /// buyer pays HIS price: the cap on the total is their sum, never the
    /// best price times the whole quantity.
    pub ladder: Vec<(f64, f64)>,
    /// Coins one unit costs to make (inputs at market): price - margin.
    pub cost_per_unit: f64,
    /// Amortized value per 6000 ticks (module doc), 0 when it never pays.
    pub value_6000: f64,
}

/// Best XP per tick of `skill` among the catalogue's methods open at
/// `level` (`None`: no method trains it there). Memoised per (skill,
/// level): static game data.
pub fn best_xp_rate(skill: &str, level: u8) -> Option<f64> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(String, u8), Option<f64>>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, (skill.to_string(), level.max(1)), || {
        let mut best: Option<f64> = None;
        for entry in crate::guide_methods::catalogue() {
            if crate::activity::band_skill(&entry.method, &entry.style).as_deref() != Some(skill) || crate::guide_methods::gate_level(&entry.method) > level.max(1) {
                continue;
            }
            let Some(rate) = crate::work_rate::prior_rate(&entry.method, level.max(1)) else { continue };
            let per_tick = rate.xp_of(skill) / WINDOW_TICKS;
            if per_tick > 0.0 && best.is_none_or(|known| per_tick > known) {
                best = Some(per_tick);
            }
        }
        best
    })
}

/// Ticks to climb `skill` from `from` to `to`: the XP between the two
/// levels over the best XP rate the climb meets (the slowest of the
/// rates read at `from` and at `to - 1`: the rate rises with the level
/// and the climb starts at the low one, so the rate at `from` alone
/// would overstate the time and the one at `to` understate it; the
/// harmonic mean of the two). `None` when no method trains the skill.
pub fn climb_ticks(skill: &str, from: u8, to: u8, xp_rate: &dyn Fn(&str, u8) -> Option<f64>) -> Option<f64> {
    if to <= from {
        return Some(0.0);
    }
    let low = xp_rate(skill, from)?;
    let high = xp_rate(skill, to.saturating_sub(1).max(from))?;
    let mean = 2.0 / (1.0 / low + 1.0 / high);
    Some(crate::sizing::xp_between(from, to) / mean)
}

/// The gates of making `item` by `leg` and the inputs nothing prices,
/// only in the skills of `own` (a kind answers the steps of its own
/// skills, the others' inputs are bought; empty: every skill).
fn gates_of(leg: &crate::obtain_legs::ProductionLeg, price: &dyn Fn(&str) -> Option<f64>, own: &[&str], depth: u8, out: &mut Vec<(String, u8)>) {
    if let Some((skill, level)) = leg.skill.as_ref().filter(|(skill, _)| own.is_empty() || own.contains(&skill.as_str())) {
        push_gate(out, skill, u8::try_from((*level).clamp(1, 99)).unwrap_or(99));
    }
    if depth == 0 {
        return;
    }
    for (input, _) in &leg.inputs {
        if price(input).is_some() {
            continue;
        }
        let legs = crate::obtain_legs::production_legs(input);
        let easiest = legs.iter().filter(|candidate| candidate.skill.is_some() && candidate.quest.is_none()).min_by_key(|candidate| candidate.skill.as_ref().map(|(_, level)| *level).unwrap_or(0));
        if let Some(candidate) = easiest {
            gates_of(candidate, price, own, depth - 1, out);
        }
    }
}

fn push_gate(out: &mut Vec<(String, u8)>, skill: &str, level: u8) {
    match out.iter_mut().find(|(known, _)| known == skill) {
        Some(known) => known.1 = known.1.max(level),
        None => out.push((skill.to_string(), level)),
    }
}

/// Net coins per 6000 ticks of a rate, `product` priced at `unit_price`
/// and every other item at `price` (an unpriced input counts free: its
/// gates are in the climb, `gates_of`); coins count 1. Units of
/// `product` made per 6000 ticks alongside.
fn flows_value(rate: &WorkRate, product: &str, unit_price: f64, price: &dyn Fn(&str) -> Option<f64>) -> (f64, f64) {
    let mut coins = 0.0;
    let mut made = 0.0;
    for (item, cause, quantity) in &rate.flows_6000 {
        let value = if item == "coins" {
            1.0
        } else if item == product {
            if *quantity > 0.0 {
                made += quantity;
            }
            unit_price
        } else if crate::work_rate::CONSUMED_CAUSES.contains(&cause.as_str()) {
            price(item).unwrap_or(0.0)
        } else {
            continue;
        };
        coins += quantity * value;
    }
    (coins, made)
}

/// Coins the offers of `ladder` (unit price, units; best price first) pay
/// above `cost` per unit, once the best `covered` units are served: every
/// buyer at his own price, never the best price for all.
pub fn ladder_value(ladder: &[(f64, f64)], cost: f64, covered: f64) -> f64 {
    let mut skip = covered.max(0.0);
    let mut total = 0.0;
    for (unit_price, units) in ladder {
        let taken = skip.min(*units);
        skip -= taken;
        total += (unit_price - cost).max(0.0) * (units - taken);
    }
    total
}

/// Investment of producing `item` for a demand of `capacity` units at
/// `unit_price` (one offer: `investment_ladder` of one rung).
pub fn investment(
    item: &str,
    unit_price: f64,
    capacity: f64,
    own: &[&str],
    level_of: &dyn Fn(&str) -> u8,
    price: &dyn Fn(&str) -> Option<f64>,
    xp_rate: &dyn Fn(&str, u8) -> Option<f64>,
    rate: RateReader,
) -> Option<Investment> {
    investment_ladder(item, &[(unit_price, capacity)], own, level_of, price, xp_rate, rate)
}

/// Investment of producing `item` for the offers of `ladder` (unit price,
/// units; best price first; module doc; `own`: the skills whose gates
/// count); the gain per tick is read at the best price, the total capped
/// by what the offers pay (`ladder_value`). `None` when no production way
/// is open to F2P bots, no rate reads, or a climb has no method.
pub fn investment_ladder(
    item: &str,
    ladder: &[(f64, f64)],
    own: &[&str],
    level_of: &dyn Fn(&str) -> u8,
    price: &dyn Fn(&str) -> Option<f64>,
    xp_rate: &dyn Fn(&str, u8) -> Option<f64>,
    rate: RateReader,
) -> Option<Investment> {
    let unit_price = ladder.first().map(|(best, _)| *best)?;
    let capacity: f64 = ladder.iter().map(|(_, units)| *units).sum();
    let legs: Vec<crate::obtain_legs::ProductionLeg> =
        crate::obtain_legs::production_legs(item).into_iter().filter(|leg| leg.skill.is_some() && leg.quest.is_none() && !leg.method.starts_with("ground:")).collect();
    let mut best: Option<Investment> = None;
    for leg in &legs {
        let mut wanted: Vec<(String, u8)> = Vec::new();
        gates_of(leg, price, own, GATE_DEPTH, &mut wanted);
        let mut gates = Vec::new();
        let mut climb = 0.0;
        let mut climbable = true;
        for (skill, level) in &wanted {
            let have = level_of(skill);
            let ticks = if have >= *level { 0.0 } else { climb_ticks(skill, have, *level, xp_rate).unwrap_or_else(|| { climbable = false; 0.0 }) };
            climb += ticks;
            gates.push(Gate { skill: skill.clone(), level: *level, climb_ticks: ticks });
        }
        if !climbable {
            continue;
        }
        let (own_skill, own_gate) = leg.skill.clone().map(|(skill, level)| (skill, u8::try_from(level.clamp(1, 99)).unwrap_or(99))).unwrap_or_default();
        let top = level_of(&own_skill).max(own_gate);
        let Some(work) = rate(&leg.method, top) else { continue };
        let (coins_6000, made_6000) = flows_value(&work, item, unit_price, price);
        if made_6000 <= 0.0 {
            continue;
        }
        let gain_per_tick = (coins_6000 / WINDOW_TICKS).max(0.0);
        let margin = coins_6000 / made_6000;
        let open = (INVESTMENT_HORIZON_TICKS - climb).max(0.0);
        let cost_per_unit = unit_price - margin;
        let total = (gain_per_tick * open).min(ladder_value(ladder, cost_per_unit, 0.0));
        let value_6000 = total / (INVESTMENT_HORIZON_TICKS / WINDOW_TICKS);
        let found = Investment {
            item: item.to_string(),
            method: leg.method.clone(),
            gates,
            climb_ticks: climb,
            gain_per_tick,
            margin,
            capacity,
            units_per_tick: made_6000 / WINDOW_TICKS,
            ladder: ladder.to_vec(),
            cost_per_unit,
            value_6000,
        };
        if best.as_ref().is_none_or(|known| found.value_6000 > known.value_6000) {
            best = Some(found);
        }
    }
    best
}

/// One alchemy the asks offer: object, unit price, units resting.
#[derive(Debug, Clone, PartialEq)]
pub struct AlchOffer {
    pub item: String,
    pub price: f64,
    pub units: f64,
}

/// Investment of the magic level that opens alchemy (user 2026-11-21,
/// « magie 55 -> alchimie »): casts per tick (`alchemy::CAST_TICKS`) on
/// the offers that pay after the runes (`Caster::profit`), the capacity
/// the units on the asks; the gate is the spell's level. Returns the
/// investment of the best such level for the bot (high alchemy at 55,
/// else low at 21).
pub fn alchemy_investment(
    level_of: &dyn Fn(&str) -> u8,
    offers: &[AlchOffer],
    price: &dyn Fn(&str) -> Option<f64>,
    xp_rate: &dyn Fn(&str, u8) -> Option<f64>,
) -> Option<Investment> {
    let have = level_of("magic");
    let mut best: Option<Investment> = None;
    for gate in [21u8, 55u8] {
        let caster = crate::alchemy::caster(gate, false, &|rune| price(rune).unwrap_or(1.0))?;
        let mut paying: Vec<(f64, f64)> = offers.iter().filter_map(|offer| caster.profit(&offer.item, offer.price).filter(|profit| *profit > 0.0).map(|profit| (profit, offer.units.max(0.0)))).collect();
        paying.sort_by(|left, right| right.0.total_cmp(&left.0));
        let Some(top) = paying.first().map(|(profit, _)| *profit) else { continue };
        let capacity: f64 = paying.iter().map(|(_, units)| *units).sum();
        let total_margin: f64 = paying.iter().map(|(profit, units)| profit * units).sum();
        let ticks = if have >= gate { 0.0 } else { climb_ticks("magic", have, gate, xp_rate)? };
        let gain_per_tick = top / crate::alchemy::CAST_TICKS;
        let open = (INVESTMENT_HORIZON_TICKS - ticks).max(0.0);
        let total = (gain_per_tick * open).min(total_margin);
        let found = Investment {
            item: "alchemy".to_string(),
            method: format!("alch:{}", caster.spell.spell),
            gates: vec![Gate { skill: "magic".to_string(), level: gate, climb_ticks: ticks }],
            climb_ticks: ticks,
            gain_per_tick,
            margin: top,
            capacity,
            units_per_tick: 1.0 / crate::alchemy::CAST_TICKS,
            ladder: paying.clone(),
            cost_per_unit: 0.0,
            value_6000: total / (INVESTMENT_HORIZON_TICKS / WINDOW_TICKS),
        };
        if best.as_ref().is_none_or(|known| found.value_6000 > known.value_6000) {
            best = Some(found);
        }
    }
    best
}

/// Coins one tick of the bot's time is worth while a climb that opens a
/// lucrative way is pending (the training line choice, A1 (a)): the way's
/// gain per tick once open, spread over the horizon the climb eats, i.e.
/// `value_6000 / 6000`. A line that costs coins but saves ticks is worth
/// it at that rate. 0 when nothing lucrative waits.
pub fn pull_per_tick(investments: &[Investment]) -> f64 {
    investments.iter().map(|found| found.value_6000 / WINDOW_TICKS).fold(0.0, f64::max)
}

/// Coins one tick of the bot's time is worth for the training line
/// choice (`brain_train::Climb::coin_rate`): its own measured gain, or
/// the pull of a pending lucrative climb when that is higher.
pub fn time_value(own_coin_rate: f64, pending: &[Investment]) -> f64 {
    own_coin_rate.max(pull_per_tick(pending))
}

/// Every object name `investment` prices for `item` (itself, its legs'
/// inputs, the inputs of the unpriced ones, to `GATE_DEPTH`): what the
/// caller fills its price table with.
pub fn priced_items(item: &str) -> Vec<String> {
    let mut out = vec![item.to_string()];
    let mut frontier = vec![item.to_string()];
    for _ in 0..GATE_DEPTH {
        let mut next = Vec::new();
        for name in &frontier {
            for leg in crate::obtain_legs::production_legs(name) {
                for (input, _) in &leg.inputs {
                    if !out.contains(input) {
                        out.push(input.clone());
                        next.push(input.clone());
                    }
                }
            }
        }
        frontier = next;
    }
    out
}

#[cfg(test)]
#[path = "investment_tests.rs"]
mod tests;
