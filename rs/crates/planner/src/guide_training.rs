//! Training guide (plan rendements R4/R6 §2, D17, D18, D20): one line per
//! skill × band × activity × site × tool × supply mode.
//!
//! - XP per 6000 ticks is DERIVED from the yield's units (`unit_xp`:
//!   units × XP of the dump), never measured;
//! - autonomous: the work alone, or with its stock inputs farmed
//!   upstream (their sources' ticks counted, their other flows and XP
//!   too, `guide_chain::best_source`); an input no source makes at the
//!   level is bought;
//! - market: stock inputs bought (current price, or a shop's, Q7), a
//!   square round trip and the measured wait per lot (as D15);
//! - coins spent convert to ticks at the best ACCESSIBLE money line of
//!   the bot (D17: price ÷ profit per 6000 ticks), the effective XP rate
//!   counts them; ticks to the band's end use the effective rate;
//! - quest shortcuts (D20): a quest's fixed XP over its measured
//!   duration, beside the methods of the band (`chains::QuestShortcut`).

use std::collections::HashMap;

use crate::guide_engine::{price_flows, Engine, InputPricing};
use crate::guide_model::{band_of, Confidence, GuideFilter, Prereqs, Supply, TrainingRow, Unrated, WINDOW_TICKS};
use crate::guide_money::{places, row_level, Place};

/// The bot's balance of one supply mode: coins and XP per 6000 ticks
/// of the row (its work and every tick it adds).
struct Balance {
    coins_6000: f64,
    xp_6000: Vec<(String, f64)>,
    flows: Vec<crate::guide_model::FlowLine>,
    prereqs: Prereqs,
}

fn add(into: &mut Vec<(String, f64)>, skill: &str, xp: f64) {
    match into.iter_mut().find(|(known, _)| known == skill) {
        Some(known) => known.1 += xp,
        None => into.push((skill.to_string(), xp)),
    }
}

/// Autonomous balance: the work's own flows, its stock inputs farmed by
/// their fastest source (else bought: no stock is assumed).
fn autonomous(engine: &Engine, place: &Place) -> Balance {
    let answer = &place.answer;
    let own = crate::guide_methods::own_product(&place.ask.method);
    let mut extra = answer.pause_per_6000.max(0.0);
    let mut xp: Vec<(String, f64)> = crate::unit_xp::derived_xp(&place.ask.method, &place.ask.style, &answer.units, place.ask.level);
    let mut prereqs = crate::guide_prereq::prereqs_of(&place.ask.method, &place.ask.style, place.ask.level, &place.ask.tool);
    let mut farmed_value = 0.0;
    let mut farmed: Vec<String> = Vec::new();
    let path: Vec<String> = own.iter().cloned().collect();
    for (item, cause, qty) in &answer.flows {
        if cause != "input" || *qty >= 0.0 {
            continue;
        }
        let Some(source) = crate::guide_chain::best_source(engine, item, place.ask.level, 1, &path) else { continue };
        let need = -qty;
        extra += need * source.ticks_per_unit;
        farmed_value += need * source.value_consumed;
        for (skill, gained) in &source.xp_per_unit {
            add(&mut xp, skill, need * gained);
        }
        crate::guide_prereq::merge(&mut prereqs, &source.prereqs);
        farmed.push(item.clone());
    }
    let scale = WINDOW_TICKS / (WINDOW_TICKS + extra);
    let (mut flows, _, revenue, cost) = price_flows(engine, answer, own.as_deref(), InputPricing::Bought, scale, &|item, cause| cause == "input" && farmed.iter().any(|known| known == item));
    for line in &mut flows {
        if line.cause == "input" && farmed.contains(&line.item) {
            line.outlet = crate::guide_model::Outlet::Farmed;
        }
    }
    Balance { coins_6000: revenue - cost + farmed_value * scale, xp_6000: xp.into_iter().map(|(skill, gained)| (skill, gained * scale)).collect(), flows, prereqs }
}

/// Market balance: the stock inputs bought, trips and waits per lot.
fn bought(engine: &Engine, place: &Place, band: i64) -> Balance {
    let row = crate::guide_money::market_row(engine, place, band);
    Balance { coins_6000: row.profit_6000, xp_6000: row.xp_6000, flows: row.flows, prereqs: row.prereqs }
}

/// XP from `level` to the band's end (its last level + 1), in XP.
pub fn xp_to_band_end(level: u8) -> f64 {
    let band = band_of(level);
    let end = (band + 5).min(99) as u32;
    let have = lifescape_gamedata::xp::xp_for_level(u32::from(level.max(1))).unwrap_or(0);
    let need = lifescape_gamedata::xp::xp_for_level(end).unwrap_or(have);
    (need - have).max(0) as f64 / 10.0
}

#[allow(clippy::too_many_arguments)]
fn row_of(skill: &str, place: &Place, band: i64, supply: Supply, balance: Balance, best_profit: Option<f64>, confidence: Confidence, filter: &GuideFilter) -> Option<TrainingRow> {
    let xp = balance.xp_6000.iter().find(|(name, _)| name == skill).map(|(_, xp)| *xp).unwrap_or(0.0);
    if xp <= 0.0 {
        return None;
    }
    let spent = (-balance.coins_6000).max(0.0);
    let converted = match best_profit {
        Some(profit) if profit > 0.0 && spent > 0.0 => spent / (profit / WINDOW_TICKS),
        _ => 0.0,
    };
    let effective = xp * WINDOW_TICKS / (WINDOW_TICKS + converted);
    let level = filter.level.unwrap_or(place.ask.level).max(place.ask.level);
    let ticks_to_band_end = (effective > 0.0).then(|| xp_to_band_end(level) / (effective / WINDOW_TICKS));
    let accessible = crate::guide_prereq::passes(&balance.prereqs, filter);
    Some(TrainingRow {
        skill: skill.to_string(),
        band,
        method: place.ask.method.clone(),
        style: place.ask.style.clone(),
        site: place.ask.site,
        tool: place.ask.tool.clone(),
        supply,
        prereqs: balance.prereqs,
        xp_6000: xp,
        coins_6000: balance.coins_6000,
        coins_per_xp: -balance.coins_6000 / xp,
        converted_ticks_6000: converted,
        effective_xp_6000: effective,
        ticks_to_band_end,
        other_xp_6000: balance.xp_6000.into_iter().filter(|(name, _)| name != skill).collect(),
        quest_xp: 0.0,
        quest_ticks: None,
        flows: balance.flows,
        confidence,
        accessible,
    })
}

/// Quest shortcut rows of `skill` (D20) for the filter's bot.
pub fn quest_rows(engine: &Engine, skill: &str, filter: &GuideFilter) -> Vec<TrainingRow> {
    let mut out = Vec::new();
    for shortcut in crate::chains::quests_for_goal(skill) {
        if shortcut.xp <= 0.0 || filter.quests_done.iter().any(|done| done == shortcut.quest) {
            continue;
        }
        let requirements = lifescape_gamedata::quest_requirements::by_key(shortcut.quest);
        let mut prereqs = Prereqs { ironman: true, ..Prereqs::default() };
        if let Some(found) = &requirements {
            prereqs.levels = found.skills.iter().map(|need| (need.skill.clone(), need.level.clamp(1, 99) as u8)).collect();
            prereqs.combat = found.combat_level.clamp(0, 126) as u8;
            prereqs.quests = found.dependencies.iter().map(|dependency| dependency.key.clone()).collect();
        }
        let ticks = (engine.world.quest_ticks)(shortcut.quest).filter(|ticks| *ticks > 0.0);
        let xp_6000 = ticks.map(|ticks| shortcut.xp * WINDOW_TICKS / ticks).unwrap_or(0.0);
        let level = filter.level.unwrap_or(1);
        let accessible = crate::guide_prereq::passes(&prereqs, filter);
        out.push(TrainingRow {
            skill: skill.to_string(),
            band: band_of(level),
            method: format!("quest:{}", shortcut.quest),
            style: String::new(),
            site: None,
            tool: String::new(),
            supply: Supply::Quest,
            prereqs,
            xp_6000,
            coins_6000: 0.0,
            coins_per_xp: 0.0,
            converted_ticks_6000: 0.0,
            effective_xp_6000: xp_6000,
            ticks_to_band_end: None,
            other_xp_6000: Vec::new(),
            quest_xp: shortcut.xp,
            quest_ticks: ticks,
            flows: Vec::new(),
            confidence: Confidence { measured: ticks.is_some(), read: if ticks.is_some() { "quest measured".to_string() } else { "quest unmeasured".to_string() }, ..Confidence::default() },
            accessible,
        });
    }
    out
}

/// The training guide under `filter`, best effective XP rate first, and
/// the activities nothing estimates.
pub fn training_guide(engine: &Engine, filter: &GuideFilter) -> (Vec<TrainingRow>, Vec<Unrated>) {
    let skills: Vec<String> = match &filter.skill {
        Some(skill) => vec![skill.clone()],
        None => crate::guide_methods::SKILLS.iter().map(|skill| skill.to_string()).collect(),
    };
    let mut best: HashMap<u8, Option<f64>> = HashMap::new();
    let mut rows = Vec::new();
    let mut unrated = Vec::new();
    for entry in &crate::guide_money::entries(engine) {
        let level = row_level(&entry.method, filter);
        if filter.level.is_some() && crate::guide_methods::gate_level(&entry.method) > level {
            continue;
        }
        let (found, error) = places(engine, &entry.method, &entry.style, level);
        if found.is_empty() {
            if !unrated.iter().any(|known: &Unrated| known.method == entry.method) {
                unrated.push(Unrated { method: entry.method.clone(), reason: error.unwrap_or_else(|| "no yield".to_string()) });
            }
            continue;
        }
        let band = band_of(level);
        for (index, place) in found.iter().enumerate() {
            let derived = crate::unit_xp::derived_xp(&place.ask.method, &place.ask.style, &place.answer.units, level);
            let trains: Vec<&String> = skills.iter().filter(|skill| derived.iter().any(|(name, xp)| name == *skill && *xp > 0.0)).collect();
            if trains.is_empty() {
                continue;
            }
            // D17 for the filter's bot; the catalogue view (no bot) reads
            // it at the band's first level (one money guide per band).
            let band_level = filter.level.unwrap_or(band.clamp(1, 99) as u8);
            let profit = *best.entry(band_level).or_insert_with(|| crate::guide_money::best_accessible_profit(engine, band_level, &filter.quests_done));
            let stock = place.answer.flows.iter().any(|(_, cause, qty)| cause == "input" && *qty < 0.0);
            for skill in trains {
                // Autonomous farming upstream rides the prior's place only.
                if !stock || index == 0 {
                    rows.extend(row_of(skill, place, band, Supply::Autonomous, autonomous(engine, place), profit, place.answer.confidence.clone(), filter));
                }
                if stock && crate::guide_money::inputs_buyable(engine, place) {
                    rows.extend(row_of(skill, place, band, Supply::Market, bought(engine, place, band), profit, place.answer.confidence.clone(), filter));
                }
            }
        }
    }
    for skill in &skills {
        rows.extend(quest_rows(engine, skill, filter));
    }
    rows.sort_by(|left, right| right.effective_xp_6000.total_cmp(&left.effective_xp_6000));
    (rows, unrated)
}
