//! Rare objects (plan-marche §4.4, lot M9): every object the fleet bids
//! for while nobody sells it, classed from the server data and the
//! planner's own legs, never from a hand list. Operator information only
//! (`orchestrator rares`): no bot reads it, and the bids stay as they are
//! (user decision Q8, 2026-11-16: a bid on an unfindable object counts
//! like any demand).
//!
//! Classes, first that holds:
//! - `ProducibleNow`: a planner production leg some fleet bot works at
//!   its live levels (and quests), every input itself producible now,
//!   sold by a keeper the fleet reaches, or by a dialogue seller: the
//!   chain exists, nobody runs it (Unblessed symbol, Leather vambraces);
//! - `ShopF2p`: an F2P keeper stocks it (rule 2026-11-15: every stocked
//!   object may be bought), with the door it sits behind and how many
//!   fleet bots pass it (Champions' Guild: 32 quest points, Q7);
//! - `OutOfReach`: a production leg exists, the fleet lacks the level,
//!   the quest or an input (adamant platebody at smithing 88);
//! - `Unfindable`: no planner leg nor keeper (Holy symbol: Brother
//!   Jered blesses it in the Monastery's upper floor, a service no
//!   planner leg runs; rune full helm: smithing 92, no keeper).

use std::collections::HashSet;

use lifescape_gamedata::obtain::{ways, WayKind};

/// Recursion bound of the input chain (ore → bar → plate is 3).
const CHAIN_DEPTH: u8 = 6;

/// Weight of a missing input or quest when the closest leg is chosen
/// (level gaps count their levels).
const INPUT_GAP: i64 = 30;

/// What one fleet bot reaches: live base levels and finished quests
/// (planner keys).
#[derive(Debug, Clone, Default)]
pub struct BotReach {
    pub account: String,
    pub levels: Vec<(String, u8)>,
    pub quests_done: Vec<String>,
}

impl BotReach {
    fn level(&self, skill: &str) -> u8 {
        self.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1)
    }

    fn quest_points(&self) -> i64 {
        lifescape_gamedata::quest_requirements::quest_points_of(&self.quests_done)
    }
}

/// The class of one rare object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RareClass {
    ProducibleNow,
    ShopF2p,
    OutOfReach,
    Unfindable,
}

impl RareClass {
    pub fn label(self) -> &'static str {
        match self {
            RareClass::ProducibleNow => "producible now",
            RareClass::ShopF2p => "F2P shop",
            RareClass::OutOfReach => "out of reach",
            RareClass::Unfindable => "unfindable",
        }
    }
}

/// One classed object with the evidence behind its class.
#[derive(Debug, Clone, PartialEq)]
pub struct RareVerdict {
    pub item: String,
    pub class: RareClass,
    pub detail: String,
}

/// Best level of `skill` across the fleet (1 for an empty fleet).
fn best_level(fleet: &[BotReach], skill: &str) -> u8 {
    fleet.iter().map(|bot| bot.level(skill)).max().unwrap_or(1)
}

fn best_quest_points(fleet: &[BotReach]) -> i64 {
    fleet.iter().map(BotReach::quest_points).max().unwrap_or(0)
}

/// Why the fleet cannot work `item`'s legs now, or the leg it works:
/// `Ok(method)` for a workable leg, `Err(reason)` for the closest miss
/// (`None` when no leg at all).
fn workable_leg(item: &str, fleet: &[BotReach], depth: u8, seen: &mut HashSet<String>) -> Result<String, Option<String>> {
    let legs = crate::obtain_legs::production_legs(item);
    if legs.is_empty() {
        return Err(None);
    }
    if depth >= CHAIN_DEPTH || !seen.insert(item.to_string()) {
        return Err(Some(format!("{item}: input chain too deep or cyclic")));
    }
    // Every miss of a leg counts (levels short, a quest nobody has, an
    // input out of reach): the leg with the smallest sum is the reason
    // shown (superheating runite needs the ore too, not only magic 43).
    let mut closest: Option<(i64, String)> = None;
    for leg in &legs {
        let mut gap = 0i64;
        let mut reasons: Vec<String> = Vec::new();
        if let Some((skill, needed)) = &leg.skill {
            let best = best_level(fleet, skill);
            if i64::from(best) < *needed {
                gap += *needed - i64::from(best);
                reasons.push(format!("needs {skill} {needed} (fleet best {best})"));
            }
        }
        if let Some(quest) = &leg.quest {
            if !fleet.iter().any(|bot| bot.quests_done.iter().any(|done| done == quest)) {
                gap += INPUT_GAP;
                reasons.push(format!("needs quest {quest} (no fleet bot has it)"));
            }
        }
        for (input, _) in &leg.inputs {
            if let Err(why) = available(input, fleet, depth + 1, seen) {
                gap += INPUT_GAP;
                reasons.push(format!("input {input}: {why}"));
            }
        }
        if reasons.is_empty() {
            seen.remove(item);
            let gate = leg.skill.as_ref().map(|(skill, level)| format!(" ({skill} {level})")).unwrap_or_default();
            return Ok(format!("{}{gate}", leg.method));
        }
        if closest.as_ref().is_none_or(|(known, _)| gap < *known) {
            closest = Some((gap, format!("{} {}", leg.method, reasons.join(", "))));
        }
    }
    seen.remove(item);
    Err(closest.map(|(_, why)| why))
}

/// An input the fleet comes by now: made at its levels, bought from a
/// keeper some bot reaches, or from a dialogue seller.
fn available(item: &str, fleet: &[BotReach], depth: u8, seen: &mut HashSet<String>) -> Result<(), String> {
    if lifescape_gamedata::obtain::merchant_route_for(item, best_quest_points(fleet)) || !lifescape_gamedata::dialogue_shops::allowed(item).is_empty() {
        return Ok(());
    }
    match workable_leg(item, fleet, depth, seen) {
        Ok(_) => Ok(()),
        Err(Some(why)) => Err(why),
        Err(None) => Err("no planner leg, no keeper".to_string()),
    }
}

/// Every keeper of `item` with its door and how many fleet bots pass it.
fn shop_detail(item: &str, fleet: &[BotReach]) -> Option<String> {
    let shops: Vec<String> = ways(item)
        .into_iter()
        .filter(|way| way.kind == WayKind::Shop)
        .map(|way| {
            let site = way.sites.first().map(|(x, z, level)| format!(" at ({x},{z},{level})")).unwrap_or_default();
            let mut gates: Vec<String> = Vec::new();
            if way.quest_points > 0 {
                let passing = fleet.iter().filter(|bot| bot.quest_points() >= way.quest_points).count();
                gates.push(format!("{} quest points: {passing}/{} bots pass", way.quest_points, fleet.len()));
            }
            if let Some(quest) = &way.quest {
                let key = quest.strip_prefix("quest_").unwrap_or(quest);
                let passing = fleet.iter().filter(|bot| bot.quests_done.iter().any(|done| done == key)).count();
                gates.push(format!("quest {key}: {passing}/{} bots", fleet.len()));
            }
            if way.sites.iter().any(|(x, z, level)| lifescape_gamedata::access::behind_boat(*x, *z, *level)) {
                gates.push("Karamja boat".to_string());
            }
            let gates = if gates.is_empty() { String::new() } else { format!(" [{}]", gates.join(", ")) };
            format!("{} {} gp{site}{gates}", way.detail, way.coins)
        })
        .collect();
    (!shops.is_empty()).then(|| shops.join("; "))
}

/// Ways the server knows that no planner leg runs (Brother Jered's
/// blessing), for the unfindable detail.
fn unrouted_ways(item: &str) -> Option<String> {
    let found: Vec<String> = ways(item).into_iter().filter(|way| way.kind != WayKind::Drop && way.kind != WayKind::Note).map(|way| way.describe()).collect();
    (!found.is_empty()).then(|| format!("server ways without a planner leg: {}", found.join("; ")))
}

/// The class of `item` for this fleet (module doc).
pub fn classify(item: &str, fleet: &[BotReach]) -> RareVerdict {
    let verdict = |class: RareClass, detail: String| RareVerdict { item: item.to_string(), class, detail };
    let mut seen = HashSet::new();
    let leg = workable_leg(item, fleet, 0, &mut seen);
    if let Ok(method) = &leg {
        return verdict(RareClass::ProducibleNow, format!("{method}, the whole chain within the fleet's levels"));
    }
    if let Some(shops) = shop_detail(item, fleet) {
        return verdict(RareClass::ShopF2p, shops);
    }
    if let Err(Some(why)) = leg {
        return verdict(RareClass::OutOfReach, why);
    }
    let reason = lifescape_gamedata::obtain::unobtainable_reason(item)
        .map(str::to_string)
        .or_else(|| unrouted_ways(item))
        .unwrap_or_else(|| "only drops no planner leg farms".to_string());
    verdict(RareClass::Unfindable, reason)
}

/// One resting order as the classifier reads it: base debug name (a
/// banknote counts as its object), account, buy side.
#[derive(Debug, Clone)]
pub struct RestingOrder {
    pub item: String,
    pub account: String,
    pub buy: bool,
}

/// Objects with at least one open bid from a counted account and no open
/// ask at all, with their bid count and distinct bidders, sorted by name.
/// `counted` leaves the staff out (AGENTS.md rule 11).
pub fn bid_only_items(orders: &[RestingOrder], counted: &dyn Fn(&str) -> bool) -> Vec<(String, usize, usize)> {
    let asked: HashSet<&str> = orders.iter().filter(|order| !order.buy).map(|order| order.item.as_str()).collect();
    let mut out: Vec<(String, usize, usize)> = Vec::new();
    let mut names: Vec<&str> = orders.iter().filter(|order| order.buy && counted(&order.account) && !asked.contains(order.item.as_str())).map(|order| order.item.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let bids: Vec<&RestingOrder> = orders.iter().filter(|order| order.buy && order.item == name && counted(&order.account)).collect();
        let mut bidders: Vec<&str> = bids.iter().map(|order| order.account.as_str()).collect();
        bidders.sort_unstable();
        bidders.dedup();
        out.push((name.to_string(), bids.len(), bidders.len()));
    }
    out
}

#[cfg(test)]
#[path = "rare_items_tests.rs"]
mod tests;
