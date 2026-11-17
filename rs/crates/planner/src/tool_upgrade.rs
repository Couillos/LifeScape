//! Best tool obtainable now, and when an upgrade pays for itself (user
//! decisions 2026-11-17, live jim_ellis: an ambition firemaker at
//! woodcutting 86 chopped yews for days with a bronze then an iron axe,
//! his wish aimed at a rune axe nobody smiths, while mithril axes rested
//! on the book at 312 and Bob sells steel).
//!
//! - Offer of a tier (`ToolOffer`): the cheapest way to have one NOW: the
//!   NPC shop (`tool_kit::kit_price`, Q7 routes), the best ask of the book
//!   (one unit: the volume of a tool), else, when nobody sells it, the
//!   bot's own make at its levels (`own_make`: a recipe leg its skill
//!   opens, inputs at their price). A tier without an offer is no target; its patient wish bid
//!   stays (`brain_wishlist`, Q8) and never holds back a tier on offer.
//! - Planned work of a family (`planned_work`): units the open nodes of
//!   the plan still have to work with that tool: an obtain whose locked
//!   method chops or mines (count minus progress), a woodcutting or
//!   mining climb (XP left / XP of its tranche method).
//! - Ticks saved (`ticks_saved`): units x (ticks per unit with the
//!   working tool - with the candidate), ticks per unit from the method
//!   prior (`method_rate::method_prior`: server roll ticks, the tool's
//!   odds or swing rate, depletion, bank walks).
//! - It pays (`upgrade_pays`): ticks saved > price / the bot's coins per
//!   tick (`market_fit::MIN_COIN_RATE` floor). Only then may a money step
//!   fund it (`tool_kit`: `financed`).
//!
//! Pure: levels, prices and the book come in as arguments.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::method_rate::{method_prior, PriorQuery};
use crate::model::{Life, NodeKind, NodeStatus};
use crate::tools::{required_tool, tool_family};

/// Where one tier can be had now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// An NPC shop (Bob's axes, Nurmof's pickaxes, the general stores).
    Shop,
    /// The best ask of the book.
    Ask,
    /// The bot's own make: the production method it works.
    Make(String),
}

/// The cheapest way to have one tier now, at `price` coins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOffer {
    pub price: i64,
    pub source: ToolSource,
}

/// The cheapest purchase of the shop price and the best ask (a tie goes
/// to the shop: the obtain takes the keeper when the ask is no cheaper,
/// `resolve_obtain` 0); the own make only for a tier nobody sells (its
/// input price leaves out the anvil time: a bronze axe stays Bob's).
pub fn cheapest(shop: Option<i64>, ask: Option<i64>, make: Option<(String, i64)>) -> Option<ToolOffer> {
    let mut offers: Vec<ToolOffer> = Vec::new();
    offers.extend(shop.map(|price| ToolOffer { price: price.max(1), source: ToolSource::Shop }));
    offers.extend(ask.map(|price| ToolOffer { price: price.max(1), source: ToolSource::Ask }));
    // `min_by_key` keeps the first of equal keys: the order above.
    let bought = offers.into_iter().min_by_key(|offer| offer.price);
    bought.or_else(|| make.map(|(method, price)| ToolOffer { price: price.max(1), source: ToolSource::Make(method) }))
}

/// The bot's own make of `item`: the cheapest recipe leg its levels open
/// (a skill leg with inputs: smithing an axe, never a drop farm), priced
/// at its inputs (`price_of`; an unpriced input voids the leg).
pub fn own_make(item: &str, level_of: &dyn Fn(&str) -> i64, price_of: &dyn Fn(&str) -> Option<i64>) -> Option<(String, i64)> {
    crate::obtain_legs::production_legs(item)
        .into_iter()
        .filter(|leg| !leg.inputs.is_empty() && leg.skill.as_ref().is_some_and(|(skill, needed)| level_of(skill) >= *needed))
        .filter_map(|leg| {
            let cost: Option<i64> = leg.inputs.iter().map(|(input, qty)| price_of(input).map(|price| price.max(0).saturating_mul((*qty).max(1)))).sum();
            cost.map(|cost| (leg.method, cost))
        })
        .min_by_key(|(_, cost)| *cost)
}

/// Work the plan still holds for one tool: `units` of `method` at
/// `level` in its skill.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedWork {
    pub method: String,
    pub level: u8,
    pub units: f64,
}

/// True when `method` works with a member of the family of `canonical`
/// (`bronze_axe`, `bronze_pickaxe`).
pub fn uses_family(method: &str, canonical: &str) -> bool {
    let family = tool_family(canonical);
    required_tool(method).is_some_and(|need| need.tools.iter().any(|tool| family.contains(tool)))
}

fn method_skill(method: &str) -> Option<String> {
    crate::method_way::method_way(method).and_then(|(_, way)| way.skill.map(|(skill, _)| skill))
}

/// Units the open nodes of the plan still work with the family of
/// `canonical` (module doc). `level_of`: live levels.
pub fn planned_work(life: &Life, canonical: &str, level_of: &dyn Fn(&str) -> u8) -> Vec<PlannedWork> {
    let mut out = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        match &node.kind {
            NodeKind::ObtainItems { count, .. } => {
                // Locked method, else (a window just ended, `EngagementEnd`
                // cleared it before the next resolve) its cheapest
                // production alternative: a purchase leg is no work.
                let produced = |method: &str| !["market:", "shop:", "dialogue:"].iter().any(|prefix| method.starts_with(prefix));
                let fallback = || {
                    node.alternatives
                        .iter()
                        .filter(|alt| produced(&alt.method) && uses_family(&alt.method, canonical))
                        .min_by_key(|alt| alt.est_ticks)
                        .map(|alt| alt.method.as_str())
                };
                let Some(method) = node.method.as_deref().or_else(fallback).filter(|method| uses_family(method, canonical)) else { continue };
                let units = (*count - node.progress_current).max(0) as f64;
                let level = method_skill(method).map(|skill| level_of(&skill)).unwrap_or(1);
                if units > 0.0 {
                    out.push(PlannedWork { method: method.to_string(), level, units });
                }
            }
            NodeKind::ReachLevel { skill, level } => {
                let now = level_of(skill).max(1);
                let method = node.method.clone().unwrap_or_else(|| crate::tranches::reach_method(skill, now));
                if !uses_family(&method, canonical) {
                    continue;
                }
                let Some(per_unit) = crate::method_way::action_xp(&method).filter(|xp| *xp > 0.0) else { continue };
                let units = crate::sizing::xp_between(now, *level) / per_unit;
                if units > 0.0 {
                    out.push(PlannedWork { method, level: now, units });
                }
            }
            _ => {}
        }
    }
    out
}

fn memo() -> &'static Mutex<HashMap<(String, u8, String), Option<f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<(String, u8, String), Option<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ticks one unit of `method`'s product takes at `level` with `tool`
/// (6000 / product per 6000 of the method prior). `None`: no prior.
pub fn ticks_per_unit(method: &str, level: u8, tool: &str) -> Option<f64> {
    let key = (method.to_string(), level, tool.to_string());
    if let Some(known) = memo().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        return *known;
    }
    let product = crate::method_way::method_way(method).map(|(product, _)| product);
    let query = PriorQuery { method: method.to_string(), level, tool: tool.to_string(), site: None };
    let found = product.zip(method_prior(&query).ok()).and_then(|(product, prior)| {
        let made: f64 = prior.flows.iter().filter(|(item, cause, qty)| *item == product && *cause == "product" && *qty > 0.0).map(|(_, _, qty)| *qty).sum();
        (made > 0.0).then(|| crate::method_rate::RATE_WINDOW_TICKS / made)
    });
    memo().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key, found);
    found
}

/// Ticks the planned work saves with `to` instead of `from` (work
/// without a prior for either tool counts nothing).
pub fn ticks_saved(work: &[PlannedWork], from: &str, to: &str) -> f64 {
    work.iter()
        .filter_map(|item| {
            let slow = ticks_per_unit(&item.method, item.level, from)?;
            let fast = ticks_per_unit(&item.method, item.level, to)?;
            Some(item.units * (slow - fast).max(0.0))
        })
        .sum()
}

/// A purchase of `price` coins in the bot's ticks (`coin_rate`, floored
/// at `market_fit::MIN_COIN_RATE`: a tick is never free).
pub fn cost_ticks(price: i64, coin_rate: f64) -> f64 {
    price.max(0) as f64 / coin_rate.max(crate::market_fit::MIN_COIN_RATE)
}

/// The upgrade pays for itself: the ticks it saves beat its price in
/// the bot's ticks (decision 2: no funding otherwise).
pub fn upgrade_pays(saved_ticks: f64, price: i64, coin_rate: f64) -> bool {
    saved_ticks > cost_ticks(price, coin_rate)
}

#[cfg(test)]
#[path = "tool_upgrade_tests.rs"]
pub(crate) mod tests;
