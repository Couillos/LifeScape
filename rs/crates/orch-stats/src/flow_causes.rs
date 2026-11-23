//! Cause of every item flow of a run (plan rendements §8, D19): the SDK
//! ledger (`item_ledger`, `coin_ledger`) names the action behind each
//! change of what the bot carries; this module turns it into the signed
//! balance the yields read, with the game data the planner uses (ways,
//! recipes, spells, fish tiers, equipment), never a hand list.
//!
//! Causes (the `cause` of a report flow, signed: + in, - out):
//! - `product` (P): a skill's success (ore, log, fish, bar, forged item,
//!   unfired then fired pot, rune, enchanted jewel, alchemy coins);
//! - `byproduct`: a failure output or an XP-less side gain (burnt fish,
//!   a gem while mining);
//! - `input` (C): a recipe input eaten (raw fish, ore, bar, clay, log
//!   burnt, bone buried, essence, the object alched or enchanted);
//! - method consumables: `ammo` (arrows fired, - ; recovered, +),
//!   `rune` (runes a spell burnt), `bait` (bait, feathers), `supply`
//!   (a carried tool the work uses up: thread), `food` (eaten);
//! - `discarded` (dropped by the bot: burnt fish, junk);
//! - `loot` (taken off the ground in a fight), `ground_spawn` (taken
//!   elsewhere);
//! - `fee` (tolls, fares, tanning), `death`, `shop_bought`, `shop_sold`,
//!   `traded` (player trade);
//! - `unattributed`: nothing explains it, an alert (objective zero).
//!
//! Bank transfers never reach here: the ledger drops changes made with
//! the bank open (internal moves, net zero).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

pub mod cause {
    pub const PRODUCT: &str = "product";
    pub const BYPRODUCT: &str = "byproduct";
    pub const INPUT: &str = "input";
    pub const AMMO: &str = "ammo";
    pub const RUNE: &str = "rune";
    pub const BAIT: &str = "bait";
    pub const SUPPLY: &str = "supply";
    pub const FOOD: &str = "food";
    pub const DISCARDED: &str = "discarded";
    pub const LOOT: &str = "loot";
    pub const GROUND_SPAWN: &str = "ground_spawn";
    pub const FEE: &str = "fee";
    pub const DEATH: &str = "death";
    pub const SHOP_BOUGHT: &str = "shop_bought";
    pub const SHOP_SOLD: &str = "shop_sold";
    pub const TRADED: &str = "traded";
    pub const UNATTRIBUTED: &str = "unattributed";

    /// Every cause, for contract checks.
    pub const ALL: [&str; 17] = [
        PRODUCT, BYPRODUCT, INPUT, AMMO, RUNE, BAIT, SUPPLY, FOOD, DISCARDED, LOOT, GROUND_SPAWN, FEE, DEATH, SHOP_BOUGHT, SHOP_SOLD,
        TRADED, UNATTRIBUTED,
    ];
}

/// One ledger flow, in game-data names.
#[derive(Debug, Clone, PartialEq)]
pub struct RawFlow {
    /// Debug name (`raw_shrimp`): the game data key.
    pub name: String,
    /// Display name (`Raw shrimps`): the report key.
    pub display: String,
    /// SDK ledger kind (`skill`, `pickup`, `eat`, `cast`, `shop`...).
    pub kind: String,
    pub delta: i64,
    /// Skills whose XP rose within the ledger window of this flow.
    pub skills: Vec<String>,
}

/// What the run was, for the causes the ledger leaves to the game data.
#[derive(Debug, Clone, Default)]
pub struct RunCtx {
    /// Script name (`melee`, `cook`...).
    pub activity: String,
    /// Planner method when the launch carried it (empty otherwise).
    pub planner_method: String,
    /// Skills whose XP rose during the run.
    pub run_skills: Vec<String>,
    /// Kills seen during the run (`run_events`).
    pub kills: u64,
    /// Debug names of the products the run made (first pass over its
    /// gains): their ways name the carried tools the work used up.
    pub made: Vec<String>,
}

impl RunCtx {
    fn trained(&self, skill: &str) -> bool {
        self.run_skills.iter().any(|known| known == skill)
    }

    /// A fight: a combat script, a fight or drop-farm method, or a kill.
    pub fn combat(&self) -> bool {
        matches!(self.activity.as_str(), "melee" | "ranged" | "magic")
            || self.planner_method.starts_with("fight:")
            || self.kills > 0
    }
}

/// Ammunition: a quiver item (wearpos 13) or a thrown weapon.
pub fn is_ammo(name: &str) -> bool {
    let thrown = ["_dart", "_knife", "_javelin", "_thrownaxe"].iter().any(|suffix| name.ends_with(suffix));
    thrown || lifescape_gamedata::equipment::by_name(name).is_some_and(|worn| lifescape_gamedata::equipment::covers_slot(&worn, 13))
}

/// A rune some spell burns.
pub fn is_rune(name: &str) -> bool {
    lifescape_gamedata::spells::all_spells().iter().any(|spell| spell.runes.iter().any(|(rune, _)| rune == name))
}

/// Bait or feathers of a fish tier.
pub fn is_bait(name: &str) -> bool {
    lifescape_gamedata::fishing::all_fish_tiers().iter().any(|tier| tier.bait == Some(name))
}

fn way_skill_in(way: &lifescape_gamedata::obtain::Way, skills: &[String]) -> bool {
    way.skill.as_ref().is_some_and(|(skill, _)| skills.iter().any(|known| known == skill))
}

/// `name` is made by a production way whose skill is in `skills`, or
/// whose planner leg runs through this very script (tanning, milling:
/// no XP).
fn made_by(name: &str, skills: &[String], activity: &str) -> bool {
    lifescape_planner::method_way::production_ways(name)
        .iter()
        .any(|way| way.is_production() && (way_skill_in(way, skills) || (way.skill.is_none() && crate::report_flows::leg_runs_here(name, activity))))
}

/// `name` is eaten by a production way whose skill is in `skills` (or
/// whose leg runs through this script): (as an input, as a tool of a
/// product the run made: thread sewn away).
fn eaten_by(name: &str, skills: &[String], activity: &str, made: &[String]) -> (bool, bool) {
    let mut input = false;
    let mut tool = false;
    for product in crate::report_flows::products_using(name).iter().chain(made.iter()) {
        for way in lifescape_planner::method_way::production_ways(product) {
            let works = way_skill_in(&way, skills) || (way.skill.is_none() && crate::report_flows::leg_runs_here(product, activity));
            if !works {
                continue;
            }
            input |= way.inputs.iter().any(|(wanted, _)| wanted == name);
            tool |= way.tools.iter().any(|wanted| wanted == name);
        }
    }
    (input, tool)
}

/// Failure output of a cooking recipe (burnt fish).
fn is_burnt(name: &str) -> bool {
    lifescape_gamedata::recipes::all_recipes(Some("cook")).iter().any(|recipe| recipe.burnt.as_deref() == Some(name))
}

/// Debug names of the products among `flows` (first pass of a report).
pub fn made_products(flows: &[RawFlow], ctx: &RunCtx) -> Vec<String> {
    let mut made: Vec<String> = flows
        .iter()
        .filter(|flow| flow.delta > 0 && matches!(flow.kind.as_str(), "skill" | "unattributed" | "cast"))
        .filter(|flow| cause_of(flow, ctx) == cause::PRODUCT)
        .map(|flow| flow.name.clone())
        .collect();
    made.sort();
    made.dedup();
    made
}

/// Cause of one item flow (coins excluded: `coin_cause`).
pub fn cause_of(flow: &RawFlow, ctx: &RunCtx) -> &'static str {
    let gain = flow.delta > 0;
    match flow.kind.as_str() {
        "shop" | "dialogue" => return if gain { cause::SHOP_BOUGHT } else { cause::SHOP_SOLD },
        "trade" => return cause::TRADED,
        "death" => return cause::DEATH,
        "dropped" => return cause::DISCARDED,
        "eat" => return cause::FOOD,
        "bury" => return cause::INPUT,
        "pickup" => {
            return if is_ammo(&flow.name) && ctx.trained("ranged") {
                cause::AMMO
            } else if ctx.combat() {
                cause::LOOT
            } else {
                cause::GROUND_SPAWN
            };
        }
        "cast" => {
            return if gain {
                cause::PRODUCT
            } else if is_rune(&flow.name) {
                cause::RUNE
            } else {
                cause::INPUT
            };
        }
        _ => {}
    }
    // `skill` (XP in the window) or `unattributed`: the game data says.
    let window: &[String] = if flow.skills.is_empty() { &ctx.run_skills } else { &flow.skills };
    if gain {
        if made_by(&flow.name, window, &ctx.activity) {
            return cause::PRODUCT;
        }
        if is_burnt(&flow.name) || flow.kind == "skill" {
            return cause::BYPRODUCT;
        }
        return cause::UNATTRIBUTED;
    }
    if is_ammo(&flow.name) && ctx.trained("ranged") {
        return cause::AMMO;
    }
    if is_rune(&flow.name) && ctx.trained("magic") {
        return cause::RUNE;
    }
    if is_bait(&flow.name) && (ctx.trained("fishing") || ctx.activity == "fishing") {
        return cause::BAIT;
    }
    // A failed action eats its input without XP (burnt fish, iron ore):
    // the run's skills stand in for the empty window.
    let (input, tool) = eaten_by(&flow.name, &ctx.run_skills, &ctx.activity, &ctx.made);
    let (window_input, window_tool) = if flow.skills.is_empty() { (false, false) } else { eaten_by(&flow.name, &flow.skills, &ctx.activity, &ctx.made) };
    if input || window_input {
        return cause::INPUT;
    }
    if tool || window_tool {
        return cause::SUPPLY;
    }
    if lifescape_gamedata::food::heal(&flow.name) > 0 {
        return cause::FOOD;
    }
    cause::UNATTRIBUTED
}

/// Cause of one coin flow of the coin ledger (`coin_ledger::kind`).
pub fn coin_cause(kind: &str, delta: i64, ctx: &RunCtx) -> &'static str {
    match kind {
        "shop" if delta > 0 => cause::SHOP_SOLD,
        "shop" | "dialogue" => cause::SHOP_BOUGHT,
        "trade" => cause::TRADED,
        "fee" => cause::FEE,
        "loot" if ctx.combat() => cause::LOOT,
        "loot" => cause::GROUND_SPAWN,
        "alch" if delta > 0 => cause::PRODUCT,
        "dropped" => cause::DISCARDED,
        _ => cause::UNATTRIBUTED,
    }
}

/// The alchemy a run cast (its confirmed spells), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alchemy {
    Low,
    High,
}

impl Alchemy {
    /// High when the run cast high alchemy, low for low alchemy only.
    pub fn of_spells(spells: &[&str]) -> Option<Alchemy> {
        if spells.contains(&"highlvl_alchemy") {
            Some(Alchemy::High)
        } else if spells.contains(&"lowlvl_alchemy") {
            Some(Alchemy::Low)
        } else {
            None
        }
    }
}

/// The provenance of a report line (`task_yields`), from its cause.
/// Trading runs keep `traded` for their market lines, never for an NPC
/// shop leg nor a fee (user 2026-11-18: the janitor's NPC sales read as
/// player trades in Item holdings); the object an alchemy run (`alch`)
/// eats reads `alch_low` / `alch_high`, its runes stay consumed.
pub fn provenance_of(cause_name: &str, delta: i64, trading: bool, alch: Option<Alchemy>) -> &'static str {
    use lifescape_sdk::provenance::Provenance;
    if cause_name == cause::INPUT && delta < 0 {
        match alch {
            Some(Alchemy::High) => return Provenance::AlchHigh.as_str(),
            Some(Alchemy::Low) => return Provenance::AlchLow.as_str(),
            None => {}
        }
    }
    if trading && !matches!(cause_name, cause::FOOD | cause::SHOP_BOUGHT | cause::SHOP_SOLD | cause::FEE) {
        return Provenance::Traded.as_str();
    }
    let provenance = match cause_name {
        cause::PRODUCT | cause::BYPRODUCT => Provenance::Produced,
        cause::INPUT | cause::AMMO | cause::RUNE | cause::BAIT | cause::SUPPLY | cause::FOOD => Provenance::Consumed,
        cause::DISCARDED => Provenance::Lost,
        cause::LOOT => Provenance::Loot,
        cause::GROUND_SPAWN => Provenance::GroundSpawn,
        cause::FEE => Provenance::Fee,
        cause::DEATH => Provenance::Death,
        cause::SHOP_BOUGHT => Provenance::ShopBought,
        cause::SHOP_SOLD => Provenance::ShopSold,
        cause::TRADED => Provenance::Traded,
        _ if delta > 0 => Provenance::Unattributed,
        _ => Provenance::Lost,
    };
    provenance.as_str()
}

/// One line of the signed balance: (display name, cause, quantity),
/// gains and losses of the same cause kept apart.
pub type BalanceLine = (String, &'static str, i64);

/// Sums flows into balance lines per (name, cause, sign), sorted.
pub fn balance(lines: &[BalanceLine]) -> Vec<BalanceLine> {
    let mut sums: BTreeMap<(String, &'static str, bool), i64> = BTreeMap::new();
    for (name, cause_name, qty) in lines {
        if *qty != 0 {
            *sums.entry((name.clone(), cause_name, *qty > 0)).or_insert(0) += qty;
        }
    }
    sums.into_iter().map(|((name, cause_name, _), qty)| (name, cause_name, qty)).collect()
}

/// XP-bearing object units of a balance (§7): products made (P, gross,
/// coins excluded: alchemy's coins are its profit, not its unit) and
/// inputs eaten (C).
pub fn object_units(lines: &[BalanceLine]) -> (BTreeMap<String, i64>, BTreeMap<String, i64>) {
    let mut products: BTreeMap<String, i64> = BTreeMap::new();
    let mut inputs: BTreeMap<String, i64> = BTreeMap::new();
    for (name, cause_name, qty) in lines {
        if *cause_name == cause::PRODUCT && *qty > 0 && name != "Coins" {
            *products.entry(name.clone()).or_insert(0) += qty;
        } else if *cause_name == cause::INPUT && *qty < 0 {
            *inputs.entry(name.clone()).or_insert(0) += -qty;
        }
    }
    (products, inputs)
}

#[cfg(test)]
#[path = "flow_causes_tests.rs"]
mod tests;
