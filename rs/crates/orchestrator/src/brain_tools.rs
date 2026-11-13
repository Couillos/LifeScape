#![forbid(unsafe_code)]
//! Tools a bot keeps off the market (user rule 2026-11-13: tools SELL;
//! a bot keeps only what it works with).
//!
//! A bot keeps ONE unit of the best tool it holds in each family its work
//! needs — the methods of its open graph nodes (gathering, fighting and
//! production) and the skills of its ambition's open milestones — plus
//! the whole stack of the consumables those methods burn (bait, feathers,
//! arrows, runes). Every other tool is ordinary surplus and sells: spare
//! copies, lower tiers of a family it holds better, tools of skills it
//! does not train. Replaces the blanket D52 tool gate (every axe, pickaxe,
//! net, … never sold by anyone), which kept tools off the market.

use lifescape_planner::{Life, MilestoneStatus, NodeStatus};

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
        for stone in ambition.milestones.iter().filter(|stone| stone.status != MilestoneStatus::Done) {
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
pub fn kept_tools(life: &Life, stacks: &[(String, i64)]) -> Vec<String> {
    let held = |name: &str| stacks.iter().any(|(held_name, qty)| held_name == name && *qty > 0);
    let (tools, consumables) = work_needs(life);
    let mut kept = Vec::new();
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

/// Every name of the needed tool families (consumables excluded): the
/// names whose spare copies beyond one unit sell.
fn single_unit_tools(life: &Life) -> Vec<String> {
    let (tools, _) = work_needs(life);
    tools.iter().flat_map(|tool| family_of(tool)).collect()
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

#[cfg(test)]
#[path = "brain_tools_tests.rs"]
mod tests;
