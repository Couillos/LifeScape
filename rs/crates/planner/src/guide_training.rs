//! Training guide (plan rendements R4/R6 §2, D17, D18, D20): one line per
//! skill × band × activity × site × tool × supply mode.
//!
//! - XP per 6000 ticks is DERIVED from the yield's units (`unit_xp`:
//!   units × XP of the dump), never measured;
//! - autonomous (D13: the bot obtains everything itself): the work
//!   alone, or with its stock inputs farmed upstream by their fastest
//!   autonomous source, recursively (their sources' ticks counted, their
//!   other flows and XP too, `guide_chain::best_source` under
//!   `Sourcing::Farm`; a by-product or a drop at its own rate: an uncut
//!   gem costs the mining of 1 roll in 256); an input with no autonomous
//!   source at the level is a condition impossible today (`SourceGap`,
//!   D22: the row stays, unpriced), never bought.
//!   The method's consumables (runes, ammo, bait, supply, food) are
//!   farmed the same way when an activity makes them at the level
//!   (decision 2026-11-16 §1: runes by runecraft, feathers from
//!   chickens), their time counted; only those nothing makes (fees,
//!   runes of an altar out of reach...) stay purchases; the line names
//!   the way of each (`routes`);
//! - market: stock inputs bought (current price, or a shop's, Q7), a
//!   square round trip and the measured wait per PURCHASE, one purchase
//!   carrying the band's inputs (`band_slots`, at least a pack; user lever
//!   2026-11-17: the climb buys its tranche at once through need bids at
//!   its volume, never a pack per market visit); what the
//!   asks and the shops hold is a condition (D22, the money guide's
//!   purchase rule). A method that spends only consumables (teleports,
//!   curses, a fight's ammo, bait: no stock input) gets one too (user
//!   2026-11-17): every consumable bought at its volume, the coins
//!   converted by D17 as any market line; none when its autonomous line
//!   already buys every consumable (nothing makes one: a copy);
//! - D22: no line is blocked; an unpriced line (a purchase nothing
//!   prices, an input without autonomous source) carries no effective
//!   rate (as an unpriced money line no profit) and ranks after the
//!   priced ones; its conditions say why (`guide_feasible`);
//! - coins spent convert to ticks at the best ACCESSIBLE money line of
//!   the bot (D17: price ÷ profit per 6000 ticks), the effective XP rate
//!   counts them; ticks to the band's end use the effective rate;
//! - quest shortcuts (D20): a quest's fixed XP over its measured
//!   duration, beside the methods of the band (`chains::QuestShortcut`).

use crate::guide_chain::{Choice, Sourcing};
use crate::guide_engine::{price_flows, Engine, InputPricing};
use crate::guide_model::{band_of, Confidence, GuideFilter, InputRoute, Prereqs, Route, Supply, TrainingRow, Unrated, WINDOW_TICKS};
use crate::guide_money::{places, row_level, Place};

/// The bot's balance of one supply mode: coins and XP per 6000 ticks
/// of the row (its work and every tick it adds).
struct Balance {
    coins_6000: f64,
    xp_6000: Vec<(String, f64)>,
    flows: Vec<crate::guide_model::FlowLine>,
    prereqs: Prereqs,
    /// False when a purchase has no price or an input no source: no
    /// coins counted (D22: never a block, the conditions say why).
    priced: bool,
    /// The way each input and consumable came by (autonomous).
    routes: Vec<InputRoute>,
}

fn add(into: &mut Vec<(String, f64)>, skill: &str, xp: f64) {
    match into.iter_mut().find(|(known, _)| known == skill) {
        Some(known) => known.1 += xp,
        None => into.push((skill.to_string(), xp)),
    }
}

/// Autonomous balance (D13: the bot obtains everything itself): the
/// work's own flows, each stock input AND each consumable it spends
/// (decision 2026-11-16 §1: runes, feathers, bait...) farmed by its
/// fastest AUTONOMOUS source (`guide_chain::choose` under
/// `Sourcing::Farm`: no purchase of an input anywhere upstream, a
/// by-product or a drop counted at its own rate), its ticks counted. An
/// input without one is a source gap (D22: impossible today, the row
/// unpriced), never a purchase in disguise; a consumable without one is
/// bought (its flow priced, its availability a condition).
fn autonomous(engine: &Engine, place: &Place) -> Balance {
    let answer = &place.answer;
    let own = crate::guide_methods::own_product(&place.ask.method);
    let mut extra = answer.pause_per_6000.max(0.0);
    let mut xp: Vec<(String, f64)> = crate::unit_xp::derived_xp(&place.ask.method, &place.ask.style, &answer.units, place.ask.level);
    let mut prereqs = crate::guide_prereq::prereqs_of(&place.ask.method, &place.ask.style, place.ask.level, &place.ask.tool);
    let mut farmed_value = 0.0;
    let mut farmed: Vec<(String, String)> = Vec::new();
    let mut unsourced: Vec<crate::guide_model::SourceGap> = Vec::new();
    // Consumables the farmed sources buy upstream, per 6000 ticks of work.
    let mut upstream: Vec<(String, String, f64)> = Vec::new();
    let mut routes: Vec<InputRoute> = Vec::new();
    let path: Vec<String> = own.iter().cloned().collect();
    for (item, cause, qty) in &answer.flows {
        if !crate::guide_chain::sourceable(item, cause, *qty) {
            continue;
        }
        let need = -qty;
        let (choice, route) = crate::guide_chain::choose(engine, item, cause, need, need, place.ask.level, 0, &path, Sourcing::Farm);
        routes.push(route);
        match choice {
            Choice::Farm(source) => {
                extra += need * source.ticks_per_unit;
                farmed_value += need * source.value_consumed;
                for (skill, gained) in &source.xp_per_unit {
                    add(&mut xp, skill, need * gained);
                }
                crate::guide_prereq::merge(&mut prereqs, &source.prereqs);
                routes.extend(source.routes.iter().map(|upstream| InputRoute { units: upstream.units * need, ..upstream.clone() }));
                upstream.extend(source.bought.iter().map(|(name, why, count)| (name.clone(), why.clone(), count * need)));
                farmed.push((item.clone(), cause.clone()));
            }
            // Priced below with the other flows (a purchase).
            Choice::Buy(_) => {}
            Choice::Missing(_) => {
                if cause == "input" {
                    if !unsourced.iter().any(|known| known.item == *item) {
                        unsourced.push(crate::guide_model::SourceGap { item: item.clone(), reason: crate::guide_chain::autonomous_gap(engine, item, place.ask.level, 1, &path) });
                    }
                    farmed.push((item.clone(), cause.clone()));
                }
            }
        }
    }
    let scale = WINDOW_TICKS / (WINDOW_TICKS + extra);
    // Farmed and unsourced flows are never priced as purchases.
    let priced = price_flows(engine, answer, own.as_deref(), InputPricing::Bought, scale, &|item, cause| farmed.iter().any(|(known, why)| known == item && why == cause));
    crate::guide_prereq::require_shop(&mut prereqs, priced.need);
    prereqs.market.extend(priced.market);
    // Upstream purchases: their lot is the window (they stack: runes...).
    for (item, cause, units) in upstream {
        let bought = engine.buy_at(&item, units);
        prereqs.market.push(engine.market_need(&item, &cause, units * scale, crate::guide_engine::whole_units(units), bought.priced));
    }
    let is_priced = priced.priced && unsourced.is_empty();
    prereqs.sources.extend(unsourced);
    let coins_6000 = if is_priced { priced.revenue - priced.cost + farmed_value * scale } else { 0.0 };
    for route in &mut routes {
        route.units *= scale;
    }
    Balance { coins_6000, xp_6000: xp.into_iter().map(|(skill, gained)| (skill, gained * scale)).collect(), flows: priced.lines, prereqs, priced: is_priced, routes }
}

/// Market balance: the stock inputs bought, one trip and one wait per
/// purchase of the band's inputs (`band_slots`, user lever 2026-11-17:
/// the climb buys its tranche at once, need bids at its volume).
fn bought(engine: &Engine, place: &Place, band: i64, skill: &str) -> Balance {
    let row = crate::guide_money::market_row_with(engine, place, band, band_slots(place, skill));
    Balance { coins_6000: row.profit_6000, xp_6000: row.xp_6000, flows: row.flows, prereqs: row.prereqs, priced: row.priced, routes: Vec::new() }
}

/// Slots of non-stackable inputs the work burns over the whole band of
/// the place's level in `skill` (the yield's inputs per XP of the skill
/// times `band_xp`): what one purchase of a market climb carries (a
/// tranche of five levels). `None` without XP of the skill or a slot
/// input.
pub fn band_slots(place: &Place, skill: &str) -> Option<f64> {
    let answer = &place.answer;
    let xp = crate::unit_xp::derived_xp(&place.ask.method, &place.ask.style, &answer.units, place.ask.level).into_iter().find(|(name, _)| name == skill).map(|(_, xp)| xp)?;
    let slots = crate::guide_engine::lots_per_6000(answer) * crate::guide_model::LOT_SLOTS;
    (xp > 0.0 && slots > 0.0).then(|| slots * band_xp(place.ask.level) / xp)
}

/// XP of the whole band of `level` (its first level to its last + 1).
pub fn band_xp(level: u8) -> f64 {
    let start = band_of(level).clamp(1, 99) as u8;
    xp_to_band_end(start)
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
    let level = filter.level.unwrap_or(place.ask.level).max(place.ask.level);
    let price_confidence = crate::guide_model::price_confidence(&balance.flows);
    let mut row = TrainingRow {
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
        converted_ticks_6000: 0.0,
        effective_xp_6000: 0.0,
        ticks_to_band_end: None,
        other_xp_6000: balance.xp_6000.into_iter().filter(|(name, _)| name != skill).collect(),
        quest_xp: 0.0,
        quest_ticks: None,
        flows: balance.flows,
        routes: balance.routes,
        confidence,
        price_confidence,
        priced: balance.priced,
    };
    // D17: coins converted to ticks; an unpriced line (a purchase nothing
    // prices, an input without source) carries no rate, as an unpriced
    // money line no profit.
    crate::guide_read::convert(&mut row, best_profit, level);
    Some(row)
}

/// Training rows of a recipe on the bot's own stock (R7, decision
/// 2026-11-17 (a)): the money guide's recipe row of a place (inputs at
/// their current sale price, D5: the coins are what transforming pays
/// over selling the stock, often a cost), read for each skill it trains,
/// coins converted at the bot's D17 (`best_profit`) and ticks to the
/// band's end from `level`. The row keeps its conditions: one lot of the
/// stock (`Prereqs::stock`), the consumables it buys; only a bot holding
/// the lot works it (`guide_feasible`). The full window's economy (the
/// rate while the bot works), never the share its stock covers: the stock
/// bounds the batch, not the rate.
pub fn stock_rows(recipe: &crate::guide_model::MoneyRow, level: u8, best_profit: Option<f64>) -> Vec<TrainingRow> {
    let mut out = Vec::new();
    for (skill, xp) in recipe.xp_6000.iter().filter(|(_, xp)| *xp > 0.0) {
        let mut row = TrainingRow {
            skill: skill.clone(),
            band: recipe.band,
            method: recipe.method.clone(),
            style: recipe.style.clone(),
            site: recipe.site,
            tool: recipe.tool.clone(),
            supply: Supply::Stock,
            prereqs: recipe.prereqs.clone(),
            xp_6000: *xp,
            coins_6000: recipe.profit_6000,
            coins_per_xp: -recipe.profit_6000 / xp,
            converted_ticks_6000: 0.0,
            effective_xp_6000: 0.0,
            ticks_to_band_end: None,
            other_xp_6000: recipe.xp_6000.iter().filter(|(name, _)| name != skill).cloned().collect(),
            quest_xp: 0.0,
            quest_ticks: None,
            flows: recipe.flows.clone(),
            routes: Vec::new(),
            confidence: recipe.confidence.clone(),
            price_confidence: recipe.price_confidence,
            priced: recipe.priced,
        };
        crate::guide_read::convert(&mut row, best_profit, level);
        out.push(row);
    }
    out
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
            prereqs.quest_points = found.quest_points;
        }
        let ticks = (engine.world.quest_ticks)(shortcut.quest).filter(|ticks| *ticks > 0.0);
        let xp_6000 = ticks.map(|ticks| shortcut.xp * WINDOW_TICKS / ticks).unwrap_or(0.0);
        let level = filter.level.unwrap_or(1);
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
            routes: Vec::new(),
            confidence: Confidence { measured: ticks.is_some(), read: if ticks.is_some() { "quest measured".to_string() } else { "quest unmeasured".to_string() }, ..Confidence::default() },
            price_confidence: "traded",
            priced: true,
        });
    }
    out
}

/// The training guide under `filter`, best effective XP rate first, and
/// the activities nothing estimates (`guide_read::finish_training` of a
/// fresh view).
pub fn training_guide(engine: &Engine, filter: &GuideFilter) -> (Vec<TrainingRow>, Vec<Unrated>) {
    crate::guide_read::finish_training(&training_view(engine, filter.level, &filter.quests_done), filter, None)
}

/// Every training row of a bot at `level` (`None`: each method at its
/// gate, the D17 price at its band's first level) with `quests_done`,
/// every skill, activity order, then the quest shortcuts of every skill
/// (`guide_read`: the reader's filter applies to it).
pub fn training_view(engine: &Engine, level: Option<u8>, quests_done: &[String]) -> crate::guide_read::TrainingView {
    let filter = GuideFilter { level, quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    let skills: Vec<String> = crate::guide_methods::SKILLS.iter().map(|skill| skill.to_string()).collect();
    let mut rows = Vec::new();
    let mut gaps = Vec::new();
    for entry in crate::guide_money::entries(engine) {
        let level = row_level(&entry.method, &filter);
        if filter.level.is_some() && crate::guide_methods::gate_level(&entry.method) > level {
            continue;
        }
        let (found, error) = engine.timed("places", || places(engine, &entry.method, &entry.style, level));
        if found.is_empty() {
            gaps.push(crate::guide_read::Gap { method: entry.method.clone(), style: entry.style.clone(), reason: error.unwrap_or_else(|| "no yield".to_string()) });
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
            let profit = crate::guide_money::best_accessible_profit(engine, band_level, &filter.quests_done);
            let stock = place.answer.flows.iter().any(|(_, cause, qty)| cause == "input" && *qty < 0.0);
            for skill in trains {
                // Autonomous farming upstream rides the prior's place only.
                let own = (!stock || index == 0).then(|| engine.timed("autonomous", || autonomous(engine, place)));
                // Consumables only (user 2026-11-17): a market line when
                // the autonomous one farms at least one of them.
                let market = stock || own.as_ref().is_some_and(|balance| balance.routes.iter().any(|route| route.route == Route::Farm));
                if let Some(balance) = own {
                    rows.extend(row_of(skill, place, band, Supply::Autonomous, balance, profit, place.answer.confidence.clone(), &filter));
                }
                if market {
                    rows.extend(row_of(skill, place, band, Supply::Market, engine.timed("bought", || bought(engine, place, band, skill)), profit, place.answer.confidence.clone(), &filter));
                }
            }
        }
    }
    for skill in &skills {
        rows.extend(quest_rows(engine, skill, &filter));
    }
    crate::guide_read::TrainingView { level, rows, gaps }
}
