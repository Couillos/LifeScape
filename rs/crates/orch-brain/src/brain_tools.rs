#![forbid(unsafe_code)]
//! Tools a bot keeps off the market (user rule 2026-11-13: tools SELL;
//! a bot keeps only what it works with; amended 2026-11-14: a bot keeps
//! its tools across goals, the permanent kit `planner::tool_kit`).
//!
//! Permanent kit (every bot, whatever its work): the best held axe and
//! pickaxe (and the best one its level works with), one hammer, one
//! tinderbox (`tool_kit::kit_keep`). Lower tiers and spare copies sell.
//!
//! A bot keeps ONE unit of the best tool it holds in each family its work
//! needs — the methods of its open graph nodes (gathering, fighting and
//! production) and the skills of its ambition's open milestones — plus
//! the whole stack of the consumables those methods burn (bait, feathers,
//! arrows, runes). Every other tool is ordinary surplus and sells: spare
//! copies, lower tiers of a family it holds better, tools of skills it
//! does not train. Replaces the blanket D52 tool gate (every axe, pickaxe,
//! net, … never sold by anyone), which kept tools off the market.

use lifescape_planner::{Life, NodeStatus};

/// Tool families a skill always works with, by its canonical (cheapest)
/// name; fishing tools come from the products (`required_tool`).
fn skill_tools(skill: &str) -> &'static [&'static str] {
    match skill {
        "woodcutting" => &["bronze_axe"],
        "mining" => &["bronze_pickaxe"],
        "firemaking" => &["tinderbox"],
        "smithing" => &["hammer"],
        "crafting" => &["needle", "chisel"],
        "ranged" => &["shortbow"],
        _ => &[],
    }
}

/// Gear one method needs: `(tool, bait)` canonical names, gather and
/// fight methods through the planner gate, productions through gamedata.
fn method_needs(method: &str, tools: &mut Vec<String>, consumables: &mut Vec<String>) {
    if let Some(need) = lifescape_planner::required_tool(method) {
        tools.extend(need.tools.iter().cloned());
        consumables.extend(need.consumables.iter().cloned());
        return;
    }
    let Some((_, tail)) = method.split_once(['/', ':']) else { return };
    let product = tail.rsplit('/').next().unwrap_or(tail);
    for way in lifescape_gamedata::obtain::ways(product) {
        if way.is_production() {
            tools.extend(way.tools.iter().cloned());
        }
    }
}

/// Members satisfying a canonical tool, best first (the family itself,
/// or the single name).
fn family_of(tool: &str) -> Vec<String> {
    lifescape_planner::tools::tool_family(tool)
}

/// Canonical tools and consumables the bot's work needs.
pub fn work_needs(life: &Life) -> (Vec<String>, Vec<String>) {
    let mut tools = Vec::new();
    let mut consumables = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        let methods = node.method.iter().cloned().chain(node.alternatives.iter().map(|alt| alt.method.clone()));
        for method in methods {
            method_needs(&method, &mut tools, &mut consumables);
        }
    }
    if let Some(ambition) = &life.ambition {
        for stone in ambition.milestones.iter().filter(|stone| lifescape_planner::ambition::drawable(stone)) {
            match &stone.target {
                lifescape_planner::ambition::Target::Level { skill, .. } => {
                    tools.extend(skill_tools(skill).iter().map(|tool| tool.to_string()));
                }
                lifescape_planner::ambition::Target::Produce { item, skill, .. } => {
                    tools.extend(skill_tools(skill).iter().map(|tool| tool.to_string()));
                    method_needs(&format!("gather:{skill}/{item}"), &mut tools, &mut consumables);
                    method_needs(&format!("make:{item}"), &mut tools, &mut consumables);
                }
                _ => {}
            }
        }
    }
    tools.sort();
    tools.dedup();
    consumables.sort();
    consumables.dedup();
    (tools, consumables)
}

/// Names the bot keeps off the market for its work: the best held member
/// of each needed tool family (one unit, see `spare_tool_units`) and
/// every held member of a needed consumable family (whole stacks).
pub fn kept_tools(life: &Life, stacks: &[(String, i64)], levels: &[(&str, i64)]) -> Vec<String> {
    let held = |name: &str| stacks.iter().any(|(held_name, qty)| held_name == name && *qty > 0);
    let (tools, consumables) = work_needs(life);
    let owned: Vec<String> = stacks.iter().filter(|(_, qty)| *qty > 0).map(|(name, _)| name.clone()).collect();
    let mut kept = lifescape_planner::tool_kit::kit_keep(levels, &owned);
    for tool in &tools {
        if let Some(best) = family_of(tool).into_iter().find(|member| held(member)) {
            kept.push(best);
        }
    }
    for consumable in &consumables {
        kept.extend(family_of(consumable).into_iter().filter(|member| held(member)));
    }
    kept.sort();
    kept.dedup();
    kept
}

/// Kept tools over what the bot may USE (plan P5, live 2026-11-17: the
/// best held axe was the one its own listing locked, so the iron axe
/// it could use sold as surplus and woodcutting blocked "missing
/// bronze_axe"): the best held member of each family, plus the best
/// member free of the bot's listings and deals (`free` stacks), so a
/// family always keeps one usable copy.
pub fn kept_usable_tools(life: &Life, stacks: &[(String, i64)], free: &[(String, i64)], levels: &[(&str, i64)]) -> Vec<String> {
    let mut kept = kept_tools(life, stacks, levels);
    kept.extend(kept_tools(life, free, levels));
    kept.sort();
    kept.dedup();
    kept
}

/// Every name of the needed tool families and of the permanent kit
/// (consumables excluded): the names whose spare copies beyond one unit
/// sell.
fn single_unit_tools(life: &Life) -> Vec<String> {
    let (tools, _) = work_needs(life);
    tools.iter().map(String::as_str).chain(lifescape_planner::tool_kit::PERMANENT_KIT).flat_map(family_of).collect()
}

/// Base levels of a live snapshot as `(skill, level)` (empty without one).
pub fn live_levels(live: &lifescape_json::Value) -> Vec<(String, i64)> {
    crate::brain_market::live_levels(live).into_iter().map(|(name, level)| (name, i64::from(level))).collect()
}

/// Borrowed view of `live_levels` for the pure rules.
pub fn level_refs(levels: &[(String, i64)]) -> Vec<(&str, i64)> {
    levels.iter().map(|(name, level)| (name.as_str(), *level)).collect()
}

/// Spare units of kept tools: a bot keeps one of each, the rest sells
/// (`(name, qty)`; 24 buckets kept for one milking method → 23 sell).
pub fn spare_tool_units(life: &Life, stacks: &[(String, i64)], kept: &[String]) -> Vec<(String, i64)> {
    let single = single_unit_tools(life);
    stacks
        .iter()
        .filter(|(name, qty)| *qty > 1 && kept.contains(name) && single.contains(name))
        .map(|(name, qty)| (name.clone(), qty - 1))
        .collect()
}

/// A tool listing to withdraw: the bot needs that tool (`kept`) and the
/// listing locks its last free copy (held units minus listed ≤ 0).
pub fn listing_locks_needed_tool(name: &str, listed_qty: i64, stacks: &[(String, i64)], kept: &[String]) -> bool {
    if !kept.iter().any(|kept_name| kept_name == name) {
        return false;
    }
    let held: i64 = stacks.iter().filter(|(held_name, _)| held_name == name).map(|(_, qty)| *qty).sum();
    held - listed_qty < 1
}

/// What an obtain met by the holdings does (tour 2 P2-7): the copies
/// the bot holds count only when free of its own listings.
#[derive(Debug, PartialEq, Eq)]
pub enum HeldVerdict {
    /// Free copies cover the need: done without launch.
    Done,
    /// Held, but the bot's own listings lock what the need wants: the
    /// listings are withdrawn, then done (live 12:18 luke_hill: its only
    /// axe listed as surplus, "n298 holds 1 bronze_axe, done without
    /// launch" four times, then woodcut blocked "missing bronze_axe").
    WithdrawListings,
    /// Not held: the obtain runs.
    Launch,
}

/// Pure verdict over the family counts: `held` (pack, bank, worn) and
/// `free` (held minus what the bot's resting sells lock).
pub fn held_verdict(count: i64, held: i64, free: i64) -> HeldVerdict {
    if free >= count {
        HeldVerdict::Done
    } else if held >= count {
        HeldVerdict::WithdrawListings
    } else {
        HeldVerdict::Launch
    }
}

/// Units of `name` the bot's own resting sell orders lock.
pub fn listed_units(account: &str, name: &str) -> i64 {
    let (Some(service), Some(id)) = (crate::market_service::shared(), crate::brain_gear::obj_id(name)) else { return 0 };
    service.resting_rows(account, id, lifescape_store::OrderSide::Sell).iter().map(|row| row.remaining.max(0)).sum()
}

/// Cancels the bot's resting sells of every member of `item`'s family;
/// returns how many closed.
fn withdraw_family_on(service: &crate::market_service::MarketService, account: &str, item: &str) -> usize {
    lifescape_planner::tools::tool_family(item)
        .iter()
        .filter_map(|name| crate::brain_gear::obj_id(name))
        .map(|id| service.cancel_open_because(account, id, lifescape_store::OrderSide::Sell, "need_held"))
        .sum()
}

/// Withdraws the bot's listings of every `items` family and, when any
/// went, hands its worker the new locks at once (`push`), never at the
/// next market round: the relaunch a second later read the old lock and
/// blocked twice more (live 2026-11-17, noah_hill/Zezima `locked:gold_ore`).
pub fn release_own_locks_with(
    service: &crate::market_service::MarketService,
    account: &str,
    items: &[String],
    push: &mut dyn FnMut(&crate::market_service::MarketService, &str),
) -> usize {
    let withdrawn: usize = items.iter().map(|item| withdraw_family_on(service, account, item)).sum();
    if withdrawn > 0 {
        push(service, account);
    }
    withdrawn
}

/// `release_own_locks_with` on the shared market, pushing through the
/// daemon's `set_locks`.
use crate::market_host::MarketHost;

pub(crate) fn release_own_locks(orch: &dyn crate::fleet::Fleet, account: &str, items: &[String]) -> usize {
    let Some(service) = crate::market_service::shared() else { return 0 };
    release_own_locks_with(&service, account, items, &mut |service, account| orch.push_locks(service, &[account.to_string()]))
}

/// Tag of a block need the bot's own market locks hold
/// (`kit::locked::LOCKED_TAG`, plan P5).
pub use crate::log_markers::LOCKED_TAG;

/// Objects a block names as locked by the bot's own orders.
pub fn locked_needs(needs: &[String]) -> Vec<String> {
    needs.iter().filter_map(|need| need.strip_prefix(LOCKED_TAG)).filter(|item| !item.is_empty()).map(str::to_string).collect()
}

/// The block's needs with the lock tag dropped (a lock no listing
/// withdrawal frees, a deal in flight: the plain need).
pub fn plain_needs(needs: &[String]) -> Vec<String> {
    needs.iter().map(|need| need.strip_prefix(LOCKED_TAG).unwrap_or(need).to_string()).collect()
}

#[cfg(test)]
#[path = "brain_tools_tests.rs"]
mod tests;
