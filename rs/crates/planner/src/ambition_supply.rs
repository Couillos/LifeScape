//! Products chosen by the fleet's SUPPLY (user 2026-11-20: « feather, raw
//! chicken, raw beef, cowhide s'accumulent de façon extrême sans aucune
//! demande. cette demande doit exister même si c'est pour train le
//! cooking, ou pêcher des truites » ; « pourquoi personne ne fait de
//! runecrafting » ; « le marché doit s'équilibrer »). Pure: the surplus
//! comes in as an argument (`market_demand::fleet_surplus`: units the
//! fleet HOLDS, banks included, beyond the open bids), one list of
//! (object, units), empty when unknown (no data, no decision: the ladder
//! picks as before).
//!
//! - A SINK is a product of the rung's skill, open at the rung level,
//!   whose production leg eats a pile: the units of its inputs in surplus
//!   (an input a skill-less leg makes counts its own inputs' surplus too:
//!   cow hides tanned into leather) less the units of the product already
//!   piled (`sink_pressure`) cover its quota. Cooks cook the raw chicken
//!   and beef, fishers fly-fish the feathers (trout, salmon), crafters
//!   sew the hides, smiths smelt the ore, runecrafters bind the essence:
//!   every consuming skill, computed from the game data, never a list.
//!   Its XP justifies it even when its coins per tick are low (the
//!   consuming skills are the market's sinks). Sinks are drawn by the
//!   seed, weighted by their pressure (the bots of one kind spread over
//!   the piles); a sink's quota is its own (`ambition_ladder::quota`).
//! - A product is WORKABLE when one of its legs' tools can be had: held
//!   by the bot, in the fleet's surplus (held beyond the bids), sold by a
//!   shop, laid on the ground or made by a skill (`tool_obtainable`). A
//!   talisman only a rare drop yields (water: wizards, 2/128) is none:
//!   12 runecrafters stayed blocked on `missing:water_talisman` while the
//!   fleet held 5 779 earth talismans.
//! - `pick_supplied`: a sink first, else the rung's workable products
//!   (heaviest bid, else the seed: `pick_product`), else the best workable
//!   product the skill makes at the rung level; none: no quota.
//! - `revise_supplied`: the rule applies to the persisted milestones too
//!   (user rule): an open production milestone whose tools nobody can get
//!   is re-picked (ladder, extension) or dropped (a bid answer); an idle
//!   ladder quota (nothing made, not running) eating no pile is re-picked
//!   when a sink exists. Returns the ids that left (the brain closes the
//!   goal one of them runs).

use lifescape_gamedata::obtain::WayKind;

use crate::ambition::{Ambition, BotNow, MilestoneStatus, Target, EXTEND_ORIGIN};
use crate::ambition_catalog::{dominated, pick_product, spec, DemandItem, Then};
use crate::ambition_ladder::{products_of, quota};
use crate::obtain_legs::{production_legs, ProductionLeg};

/// Units of `item` in the fleet surplus (0 when absent).
pub fn surplus_of(surplus: &[(String, f64)], item: &str) -> f64 {
    surplus.iter().filter(|(known, _)| known == item).map(|(_, units)| units.max(0.0)).sum()
}

/// The legs of `item` that `skill` trains, no other way dominating them.
fn skill_legs(item: &str, skill: &str) -> Vec<ProductionLeg> {
    let legs = production_legs(item);
    legs.iter().filter(|leg| leg.skill.as_ref().is_some_and(|(trained, _)| trained == skill) && !dominated(leg, &legs)).cloned().collect()
}

/// A tool the bot can come by: held, held by the fleet beyond the bids,
/// sold by a shop (or a dialogue), laid on the ground, or made by a
/// skill. A drop or a one-off quest grant is none.
pub fn tool_obtainable(tool: &str, held: &dyn Fn(&str) -> i64, surplus: &[(String, f64)]) -> bool {
    if held(tool) > 0 || surplus_of(surplus, tool) > 0.0 {
        return true;
    }
    let sold = lifescape_gamedata::obtain::ways(tool).iter().any(|way| matches!(way.kind, WayKind::Shop | WayKind::Dialogue | WayKind::Ground));
    sold || production_legs(tool).iter().any(|leg| leg.skill.is_some())
}

/// True when `skill` can make `item` with tools the bot can come by
/// (`tool_obtainable`); always true without surplus data or when the
/// skill has no leg for it (nothing to judge).
pub fn product_workable(item: &str, skill: &str, held: &dyn Fn(&str) -> i64, surplus: &[(String, f64)]) -> bool {
    if surplus.is_empty() {
        return true;
    }
    let legs = skill_legs(item, skill);
    legs.is_empty() || legs.iter().any(|leg| leg.tools.iter().all(|tool| tool_obtainable(tool, held, surplus)))
}

/// Units of `input` the surplus supplies: its own, plus what a skill-less
/// leg makes of its inputs' surplus (cow hides tanned into leather).
fn input_units(input: &str, surplus: &[(String, f64)]) -> f64 {
    let converted = production_legs(input)
        .iter()
        .filter(|leg| leg.skill.is_none() && !leg.inputs.is_empty())
        .map(|leg| leg.inputs.iter().map(|(raw, per_unit)| surplus_of(surplus, raw) / (*per_unit).max(1) as f64).fold(f64::INFINITY, f64::min))
        .fold(0.0, f64::max);
    surplus_of(surplus, input) + converted
}

/// Pile a product of `skill` eats: the units its best leg could make of
/// the input in surplus (`input_units` ÷ units per product), less the
/// units of the product already piled. ≤ 0: it eats no pile.
pub fn sink_pressure(item: &str, skill: &str, surplus: &[(String, f64)]) -> f64 {
    let made = skill_legs(item, skill)
        .iter()
        .map(|leg| leg.inputs.iter().map(|(input, per_unit)| input_units(input, surplus) / (*per_unit).max(1) as f64).fold(0.0, f64::max))
        .fold(0.0, f64::max);
    made - surplus_of(surplus, item)
}

/// One product chosen for a rung.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub item: String,
    pub count: i64,
    /// The product eats a pile covering its quota (`sink_pressure`).
    pub sink: bool,
}

/// One value in [0, 1) from a seed (xorshift, like `pick_product`).
fn seeded_fraction(seed: u64) -> f64 {
    let mut word = seed.max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    (word % 1_000_000) as f64 / 1_000_000.0
}

/// Sinks of `skill` open at `level`: (product, its quota, its pressure),
/// workable only, the pressure covering the quota.
pub fn sinks(skill: &str, level: u8, held: &dyn Fn(&str) -> i64, surplus: &[(String, f64)]) -> Vec<(String, i64, f64)> {
    if surplus.is_empty() {
        return Vec::new();
    }
    products_of(skill)
        .into_iter()
        .filter(|product| product.level <= level && product_workable(&product.item, skill, held, surplus))
        .filter_map(|product| {
            let count = quota(std::slice::from_ref(&product), level);
            let pressure = sink_pressure(&product.item, skill, surplus);
            (pressure >= count as f64).then(|| (product.item, count, pressure))
        })
        .collect()
}

/// Product of a rung of `skill` at `level` (module doc): a sink drawn by
/// the seed weighted by pressure; else the rung's workable `items`
/// (`pick_product`: heaviest bid, else the seed); else the highest
/// workable product open at the level; `None`: nothing workable. Without
/// surplus data: `pick_product` over `items`, as before.
#[allow(clippy::too_many_arguments)]
pub fn pick_supplied(items: &[&'static str], count: i64, skill: &str, level: u8, seed: u64, demand: &[DemandItem], surplus: &[(String, f64)], held: &dyn Fn(&str) -> i64) -> Option<Pick> {
    if surplus.is_empty() {
        let item = pick_product(items, seed, demand);
        return (!item.is_empty()).then(|| Pick { item: item.to_string(), count, sink: false });
    }
    let found = sinks(skill, level, held, surplus);
    let total: f64 = found.iter().map(|(_, _, pressure)| *pressure).sum();
    if total > 0.0 {
        let mut target = seeded_fraction(seed) * total;
        for (item, sink_count, pressure) in &found {
            target -= pressure;
            if target < 0.0 {
                return Some(Pick { item: item.clone(), count: *sink_count, sink: true });
            }
        }
        let (item, sink_count, _) = found.last()?;
        return Some(Pick { item: item.clone(), count: *sink_count, sink: true });
    }
    let workable: Vec<&'static str> = items.iter().copied().filter(|item| product_workable(item, skill, held, surplus)).collect();
    if !workable.is_empty() {
        return Some(Pick { item: pick_product(&workable, seed, demand).to_string(), count, sink: false });
    }
    products_of(skill)
        .into_iter()
        .filter(|product| product.level <= level && product_workable(&product.item, skill, held, surplus))
        .max_by(|left, right| left.level.cmp(&right.level).then_with(|| right.item.cmp(&left.item)))
        .map(|product| Pick { item: product.item, count, sink: false })
}

/// Apply the supply rule to the persisted milestones (module doc).
/// `running`: the milestone the bot's goal runs now (never re-picked for
/// a sink; still re-picked when its tools are out of reach). Returns the
/// ids of the milestones that left the ambition.
pub fn revise_supplied(ambition: &mut Ambition, now: &BotNow, seed: u64, surplus: &[(String, f64)], running: Option<&str>) -> Vec<String> {
    let Some(found) = spec(&ambition.kind).filter(|_| !surplus.is_empty()) else { return Vec::new() };
    let mut gone = Vec::new();
    let mut index = 0;
    while index < ambition.milestones.len() {
        let stone = &ambition.milestones[index];
        let Target::Produce { item, count, skill } = &stone.target else {
            index += 1;
            continue;
        };
        let (item, count, skill) = (item.clone(), *count, skill.clone());
        let workable = product_workable(&item, &skill, now.held, surplus);
        let ladder = stone.origin == "ladder" || stone.origin == EXTEND_ORIGIN;
        let idle = stone.progress.0 <= 0 && running != Some(stone.id.as_str());
        let open = stone.status != MilestoneStatus::Done;
        let wants_sink = ladder && idle && workable && sink_pressure(&item, &skill, surplus) < count as f64;
        if !open || (workable && !wants_sink) {
            index += 1;
            continue;
        }
        let stone_id = stone.id.clone();
        let rung = found.ladder.iter().find(|rung| matches!(rung.then, Then::Produce(items, _, _) if items.contains(&item.as_str())));
        let pick = match (ladder, rung.map(|rung| (rung.level, rung.then))) {
            (true, Some((level, Then::Produce(items, rung_count, rung_skill)))) => {
                let bound = level.max((now.level_of)(rung_skill));
                pick_supplied(items, rung_count, rung_skill, bound, seed ^ (index as u64 + 1).wrapping_mul(0x9E37_79B9), &[], surplus, now.held)
                    .filter(|pick| pick.item != item && (pick.sink || !workable))
                    .map(|pick| Target::Produce { item: pick.item, count: pick.count, skill: rung_skill.to_string() })
            }
            _ => None,
        };
        match pick {
            Some(target) => {
                let origin = ambition.milestones[index].origin.clone();
                crate::ambition::swap_milestone(ambition, index, target, &origin);
                gone.push(stone_id);
                index += 1;
            }
            None if !workable => {
                ambition.milestones.remove(index);
                gone.push(stone_id);
            }
            None => index += 1,
        }
    }
    gone
}

#[cfg(test)]
#[path = "ambition_supply_tests.rs"]
mod tests;
