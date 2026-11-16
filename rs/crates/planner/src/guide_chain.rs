//! Chains of the guides (plan rendements D16): farm the inputs, then
//! transform them (ore -> bar -> object), every stage compared to
//! selling at that stage.
//!
//! One unit of a stage's product costs the stage's own work (6000 ticks
//! per its measured or prior rate of that product, pauses included) plus,
//! for every input, the input's quantity times the ticks of one unit of
//! the input's fastest SOURCE (a farm activity or another chain, at most
//! `MAX_CHAIN_DEPTH` deep, never through an object already in the path).
//! An input no source can make at the level is bought (D15 prices). The
//! value of a unit counts every other flow of every stage over its time
//! share (a fight's other loot, a smelter's coal...): only the farmed
//! intermediates cancel. Selling at a stage pays its product's sale price
//! (D12/Q3) over the ticks of that stage and its upstream.

use crate::guide_engine::{Engine, InputPricing};
use crate::guide_model::{RateAsk, Stage, MAX_CHAIN_DEPTH};
use crate::guide_model::Prereqs;

/// Style a source fight is read in (its drops do not depend on it).
pub const SOURCE_FIGHT_STYLE: &str = "melee:attack";

/// One unit of a chain node's product.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainUnit {
    pub method: String,
    pub product: String,
    /// Ticks for one unit, upstream included.
    pub ticks_per_unit: f64,
    /// Coins of one unit when it is consumed downstream (every other
    /// flow of the node and its upstream, its product not sold).
    pub value_consumed: f64,
    /// XP per unit, per skill, upstream included.
    pub xp_per_unit: Vec<(String, f64)>,
    /// Stages upstream first, this one last.
    pub stages: Vec<Stage>,
    pub prereqs: Prereqs,
    /// Some stage rests on a measure.
    pub measured: bool,
    /// Inputs bought (no source at the level), per unit.
    pub bought: Vec<(String, f64)>,
}

impl ChainUnit {
    /// Coins of one unit sold at this stage.
    pub fn value_sold(&self, engine: &Engine) -> f64 {
        self.value_consumed + engine.sale(&self.product).price
    }
}

fn add_xp(into: &mut Vec<(String, f64)>, skill: &str, xp: f64) {
    match into.iter_mut().find(|(known, _)| known == skill) {
        Some(known) => known.1 += xp,
        None => into.push((skill.to_string(), xp)),
    }
}

/// Planner activities that can make `item` (its production legs keyed by
/// activity, trades aside), with the style their fight is read in.
pub fn sources(item: &str) -> Vec<(String, String)> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<(String, String)>>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, item.to_string(), || compute_sources(item))
}

fn compute_sources(item: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for leg in crate::obtain_legs::production_legs(item) {
        if ["market:", "shop:", "dialogue:"].iter().any(|prefix| leg.method.starts_with(prefix)) {
            continue;
        }
        let activity = crate::activity::activity_of_method(&leg.method);
        let style = if activity.starts_with("fight:melee/") { SOURCE_FIGHT_STYLE.to_string() } else { String::new() };
        if !out.iter().any(|(known, _)| *known == activity) {
            out.push((activity, style));
        }
    }
    out
}

/// One unit of `product` made by `method` for a bot at `level` in every
/// skill; `path` holds the objects downstream (no cycle). `Err` when the
/// method cannot make it at that level.
pub fn chain_unit(engine: &Engine, method: &str, style: &str, product: &str, level: u8, depth: usize, path: &[String]) -> Result<ChainUnit, String> {
    if crate::guide_methods::gate_level(method) > level {
        return Err(format!("{method} needs level {}", crate::guide_methods::gate_level(method)));
    }
    let tool = crate::guide_methods::tool_at(method, style, level);
    let ask = RateAsk { method: method.to_string(), style: style.to_string(), site: None, tool: tool.clone(), level };
    let answer = engine.answer(&ask)?;
    let rate = answer.gained(product);
    if rate <= 0.0 {
        return Err(format!("{method} yields no {product}"));
    }
    let own_ticks = (crate::guide_model::WINDOW_TICKS + answer.pause_per_6000.max(0.0)) / rate;
    let mut ticks = own_ticks;
    let mut value = 0.0;
    let mut xp: Vec<(String, f64)> = Vec::new();
    let mut stages: Vec<Stage> = Vec::new();
    let mut prereqs = crate::guide_prereq::prereqs_of(method, style, level, &tool);
    let mut measured = answer.confidence.measured;
    let mut bought: Vec<(String, f64)> = Vec::new();
    let mut downstream: Vec<String> = path.to_vec();
    downstream.push(product.to_string());
    for (item, cause, qty) in &answer.flows {
        if item == product && *qty > 0.0 {
            continue;
        }
        let per_unit = qty / rate;
        if cause == "input" && *qty < 0.0 && depth < MAX_CHAIN_DEPTH {
            let need = -per_unit;
            let upstream = best_source(engine, item, level, depth + 1, &downstream);
            match upstream {
                Some(source) => {
                    ticks += need * source.ticks_per_unit;
                    value += need * source.value_consumed;
                    for (skill, gained) in &source.xp_per_unit {
                        add_xp(&mut xp, skill, need * gained);
                    }
                    for stage in &source.stages {
                        if !stages.iter().any(|known| known.method == stage.method && known.product == stage.product) {
                            stages.push(stage.clone());
                        }
                    }
                    crate::guide_prereq::merge(&mut prereqs, &source.prereqs);
                    measured |= source.measured;
                    bought.extend(source.bought.iter().map(|(name, count)| (name.clone(), count * need)));
                }
                None => {
                    value -= need * engine.buy(item).price;
                    bought.push((item.clone(), need));
                }
            }
            continue;
        }
        value += per_unit * engine.flow_price(item, cause, *qty, InputPricing::Stock).price;
    }
    let level_xp = crate::unit_xp::derived_xp(method, style, &answer.units, level);
    for (skill, gained) in level_xp {
        add_xp(&mut xp, &skill, gained / rate);
    }
    let unit = ChainUnit { method: method.to_string(), product: product.to_string(), ticks_per_unit: ticks, value_consumed: value, xp_per_unit: xp, stages: Vec::new(), prereqs, measured, bought };
    let sold = unit.value_sold(engine);
    stages.push(Stage { method: method.to_string(), product: product.to_string(), ticks_per_unit: ticks, profit_6000: sold / ticks * crate::guide_model::WINDOW_TICKS });
    Ok(ChainUnit { stages, ..unit })
}

/// Fastest source of one unit of `item` at `level` (fewest ticks per
/// unit), `None` when none can make it.
pub fn best_source(engine: &Engine, item: &str, level: u8, depth: usize, path: &[String]) -> Option<ChainUnit> {
    if path.iter().any(|known| known == item) {
        return None;
    }
    engine.source_memo((item.to_string(), level, depth), || compute_best_source(engine, item, level, depth, path))
}

fn compute_best_source(engine: &Engine, item: &str, level: u8, depth: usize, path: &[String]) -> Option<ChainUnit> {
    sources(item)
        .into_iter()
        .filter_map(|(method, style)| chain_unit(engine, &method, &style, item, level, depth, path).ok())
        .min_by(|left, right| left.ticks_per_unit.total_cmp(&right.ticks_per_unit))
}
