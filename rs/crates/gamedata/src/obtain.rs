//! Brain-facing obtain index: for any F2P object, every way to get it on
//! an F2P world, with what the way needs (skill level, inputs, tools,
//! coins, quest) and where it happens (F2P sites). Built on
//! `item_sources` (which says WHICH ways exist) plus the location and
//! rate tables (drops, spawns, stations, shop openers).
//!
//! Also owns the economy rules of PRD3 (decision D-A):
//! - `producible_f2p`: made or gathered by a player (recipe, skill
//!   harvest, mixing, spell, item-on-item script) from inputs that are
//!   themselves obtainable in F2P;
//! - `merchant_buy_allowed`: an NPC-shop purchase is legal only for an
//!   F2P object that is NOT producible in F2P.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::item_sources::{sources_for_name, ItemSource};

/// Coarse family of a way, for display and for the planner's routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WayKind {
    /// Recipe at a station or with tools (smith, cook, spin, craft…).
    Make,
    /// Skill harvest of a resource node (tree, rock, fishing spot).
    Gather,
    /// Non-skill harvest: shear, milk, pick, fill, tan, NPC service.
    Harvest,
    /// NPC death drop.
    Drop,
    /// Static ground spawn.
    Ground,
    /// NPC shop.
    Shop,
    /// NPC dialogue sale.
    Dialogue,
    /// Scripted grant: quest reward, item-on-item, loc search, gift.
    Script,
    /// Spell cast on an input.
    Spell,
    /// Mixed or dyed from inputs.
    Mix,
    /// Left after eating or drinking something.
    Leftover,
    /// Banknote of the base object (bank withdrawal).
    Note,
}

/// One concrete way to obtain an object.
#[derive(Debug, Clone)]
pub struct Way {
    pub kind: WayKind,
    /// What exactly: recipe row, NPC, shop, spell, loc…
    pub detail: String,
    /// Skill and level the way needs.
    pub skill: Option<(String, i64)>,
    /// Objects consumed per action.
    pub inputs: Vec<(String, i64)>,
    /// Objects carried but not consumed.
    pub tools: Vec<String>,
    /// Coins paid per action (shop price, dialogue cost, tanning fee).
    pub coins: i64,
    /// Drops per kill (drops only).
    pub rate: Option<f64>,
    /// Quantity range per action/drop when known.
    pub qty: Option<(i64, i64)>,
    /// NPC combat level (drops only).
    pub combat: Option<i64>,
    /// F2P quest the way needs (directory key, `quest_dragon`).
    pub quest: Option<String>,
    /// F2P sites `(x, z, level)` where it happens; empty = anywhere
    /// (item-on-item, spells) or unknown.
    pub sites: Vec<(i64, i64, i64)>,
    /// Server script file (scripted grants and dialogue sales).
    pub file: String,
    /// Odds that one action yields (`recipes::success_odds`: smelting
    /// iron 0.5, the ore lost on a miss); 1.0 for every certain way.
    pub success: f64,
}

impl Way {
    /// Quests the way's script file reads (coarse hint, see
    /// `script_sources::file_quests`); empty for data-driven ways.
    pub fn quest_hints(&self) -> Vec<String> {
        if self.file.is_empty() {
            return Vec::new();
        }
        crate::script_sources::file_quests(&self.file)
    }
}

impl Way {
    fn new(kind: WayKind, detail: impl Into<String>) -> Self {
        Way {
            kind,
            detail: detail.into(),
            skill: None,
            inputs: Vec::new(),
            tools: Vec::new(),
            coins: 0,
            rate: None,
            qty: None,
            combat: None,
            quest: None,
            sites: Vec::new(),
            file: String::new(),
            success: 1.0,
        }
    }

    /// Units one action yields on average: the quantity range's middle
    /// (1 when unknown) times the success odds. Inputs per unit made are
    /// `per action / expected_out` (2 iron ore per iron bar).
    pub fn expected_out(&self) -> f64 {
        let per_action = self.qty.map(|(low, high)| (low + high) as f64 / 2.0).filter(|qty| *qty > 0.0).unwrap_or(1.0);
        per_action * self.success
    }

    /// True for ways that produce the object from work (D-A families).
    pub fn is_production(&self) -> bool {
        match self.kind {
            WayKind::Make | WayKind::Gather | WayKind::Harvest | WayKind::Spell | WayKind::Mix => true,
            // A skill script or an item use with inputs is a recipe
            // (uncooked stew, chocolate cake); quest steps, NPC gifts and
            // cupboards are not production.
            // (Skill labels may take their inputs in the calling block:
            // `make_uncooked_stew` deletes nothing itself.)
            WayKind::Script => {
                self.quest.is_none()
                    && (self.file.starts_with("skill_")
                        || (!self.inputs.is_empty() && (self.detail.starts_with("[opheld") || self.detail.starts_with("[oplocu"))))
            }
            _ => false,
        }
    }

    /// One readable line: `drop goblin (combat 2) 2.34%/kill x1, 12 F2P sites`.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("{:?}", self.kind).to_lowercase(), self.detail.clone()];
        if let Some((skill, level)) = &self.skill {
            parts.push(format!("{skill} {level}"));
        }
        if let Some(combat) = self.combat {
            parts.push(format!("combat {combat}"));
        }
        if let Some(rate) = self.rate {
            parts.push(format!("{:.2}%/kill", rate * 100.0));
        }
        if let Some((low, high)) = self.qty {
            parts.push(if low == high { format!("x{low}") } else { format!("x{low}-{high}") });
        }
        if self.success < 1.0 {
            parts.push(format!("{:.0}% success", self.success * 100.0));
        }
        if !self.inputs.is_empty() {
            let inputs: Vec<String> = self.inputs.iter().map(|(name, count)| format!("{count} {name}")).collect();
            parts.push(format!("from {}", inputs.join(" + ")));
        }
        if !self.tools.is_empty() {
            parts.push(format!("with {}", self.tools.join(", ")));
        }
        if self.coins > 0 {
            parts.push(format!("{} gp", self.coins));
        }
        if let Some(quest) = &self.quest {
            parts.push(format!("quest {quest}"));
        }
        match self.sites.len() {
            0 => {}
            1 => parts.push(format!("at {:?}", self.sites[0])),
            count => parts.push(format!("{count} sites, e.g. {:?}", self.sites[0])),
        }
        parts.join(" | ")
    }
}

fn npc_sites(npc_id: usize) -> Vec<(i64, i64, i64)> {
    crate::f2p::f2p_npc_spawns(Some(npc_id)).iter().map(|spawn| (spawn.x, spawn.z, spawn.level)).collect()
}

fn npc_sites_by_name(name: &str) -> Vec<(i64, i64, i64)> {
    crate::npcs_by_name(name).iter().flat_map(|npc| npc_sites(npc.npc_id)).collect()
}

fn station_sites(kind: &str) -> Vec<(i64, i64, i64)> {
    crate::stations::stations_of(kind)
        .iter()
        .filter(|station| crate::free_zones::is_free(station.x, station.z))
        .map(|station| (station.x, station.z, station.level))
        .collect()
}

fn price_of(name: &str) -> i64 {
    crate::objs_by_name(name).into_iter().next().map(|obj| obj.price.max(1)).unwrap_or(1)
}

/// Station kind, tools and skill of a dumped recipe kind.
fn recipe_frame(kind: &str) -> (Option<&'static str>, &'static [&'static str], &'static str) {
    match kind {
        "smith" => (Some("anvil"), &["hammer"], "smithing"),
        "smelt" => (Some("furnace"), &[], "smithing"),
        "cook" => (Some("range"), &[], "cooking"),
        "leather" => (None, &["needle", "thread"], "crafting"),
        "gem" => (None, &["chisel"], "crafting"),
        "spin" => (Some("spinningwheel"), &[], "crafting"),
        "pottery_shape" => (Some("potterywheel"), &[], "crafting"),
        "pottery_fire" => (Some("potteryoven"), &[], "crafting"),
        "jewellery" => (Some("furnace"), &[], "crafting"),
        "string" => (None, &[], "crafting"),
        "cook_prep" => (None, &[], "cooking"),
        "runecraft" => (Some("ruin"), &[], "runecraft"),
        "firemaking" => (None, &["tinderbox"], "firemaking"),
        "mill" => (Some("hopper"), &[], "cooking"),
        _ => (None, &[], "crafting"),
    }
}

/// Recipe inputs that are carried, never consumed: jewellery moulds and
/// runecraft talismans (server `jewellery.rs2`, `runecraft.rs2`).
fn carried_tool(name: &str) -> bool {
    name.ends_with("_mould") || name.ends_with("_talisman")
}

fn make_way(kind: &str, row: &str, level: i64, inputs: Vec<(String, i64)>, station: Option<String>) -> Way {
    let (default_station, tools, skill) = recipe_frame(kind);
    let mut way = Way::new(WayKind::Make, format!("{kind} {row}"));
    way.skill = Some((skill.to_string(), level.max(1)));
    // Tools ride the input list in `extra_recipes` (tinderbox, the
    // jewellery mould, the altar's talisman): keep consumed inputs and
    // carried tools apart. A mould casts again and again; a talisman opens
    // the ruins and stays in the pack (`runecraft.rs2` never deletes it).
    let (kept, consumed): (Vec<(String, i64)>, Vec<(String, i64)>) =
        inputs.into_iter().partition(|(name, _)| tools.contains(&name.as_str()) || carried_tool(name));
    way.inputs = consumed;
    way.tools = tools.iter().map(|tool| tool.to_string()).collect();
    for (name, _) in kept {
        if !way.tools.contains(&name) {
            way.tools.push(name);
        }
    }
    way.success = crate::recipes::success_odds(row);
    if let Some(station) = station.as_deref().or(default_station) {
        way.sites = station_sites(station);
    }
    way
}

/// Every way to obtain `item` (debug name) on an F2P world.
pub fn ways(item: &str) -> Vec<Way> {
    if let Some(cached) = index().ways.get(item) {
        return cached.clone();
    }
    compute_ways(item)
}

fn compute_ways(item: &str) -> Vec<Way> {
    sources_for_name(item).into_iter().filter_map(|source| way_of(item, source)).collect()
}

/// Ways of every F2P object plus the obtainable closure, computed once.
struct Index {
    ways: HashMap<String, Vec<Way>>,
    obtainable: HashSet<String>,
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut ways = HashMap::new();
        for obj in crate::f2p::f2p_objs() {
            if let Some(name) = obj.name {
                ways.insert(name.to_string(), compute_ways(name));
            }
        }
        // Fixed point: an object is obtainable once one of its ways
        // needs nothing, or only inputs already known obtainable.
        let mut obtainable: HashSet<String> = HashSet::new();
        loop {
            let mut grew = false;
            for (name, list) in &ways {
                if obtainable.contains(name) {
                    continue;
                }
                let reachable = list.iter().any(|way| match way.kind {
                    WayKind::Drop => way.rate.is_some_and(|rate| rate > 0.0),
                    _ => way.inputs.iter().all(|(input, _)| obtainable.contains(input)),
                });
                if reachable {
                    obtainable.insert(name.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        Index { ways, obtainable }
    })
}

fn way_of(item: &str, source: ItemSource) -> Option<Way> {
    let way = match source {
        ItemSource::Shop { shop } => {
            let access = crate::f2p::f2p_shop_access(&shop)?;
            let mut way = Way::new(WayKind::Shop, shop);
            way.coins = price_of(item);
            way.quest = access.quest;
            way.sites = access.npcs.iter().flat_map(|npc| npc_sites(*npc)).collect();
            way
        }
        ItemSource::Spawn { x, z, level } => {
            let mut way = Way::new(WayKind::Ground, "ground spawn");
            way.sites = vec![(x, z, level)];
            way
        }
        ItemSource::Recipe { kind, row, level } => {
            let recipe = crate::recipes::all_recipes(Some(&kind)).into_iter().find(|entry| entry.source == row)?;
            make_way(&kind, &row, level, recipe.inputs.clone(), None)
        }
        ItemSource::Extra { kind, row, level } => {
            let recipe = crate::extra_recipes::by_kind(&kind).into_iter().find(|entry| entry.source == row)?;
            make_way(&kind, &row, level, recipe.inputs.clone(), recipe.station.clone())
        }
        ItemSource::Burnt { row } => {
            let mut way = Way::new(WayKind::Make, format!("burnt while cooking {row}"));
            way.sites = station_sites("range");
            way
        }
        ItemSource::Drop { npc, npc_id, rate, qty_min, qty_max } => {
            let mut way = Way::new(WayKind::Drop, npc);
            way.rate = Some(rate);
            way.qty = (qty_min >= 0).then_some((qty_min, qty_max.max(qty_min)));
            way.combat = crate::npc_by_id(npc_id).map(|row| row.vislevel);
            way.sites = npc_sites(npc_id);
            way
        }
        ItemSource::BankNote { base_id } => {
            let base = crate::obj_by_id(base_id).and_then(|obj| obj.name).unwrap_or("?");
            let mut way = Way::new(WayKind::Note, format!("note of {base}"));
            way.inputs = vec![(base.to_string(), 1)];
            way
        }
        ItemSource::Woodcut { level } => gather_way("woodcutting", "woodcut", item, level, vec!["bronze_axe".to_string()]),
        ItemSource::Mine { level } => {
            let mut way = gather_way("mining", "mining", item, level, vec!["bronze_pickaxe".to_string()]);
            way.quest = gather_quest(item).map(str::to_string);
            way
        }
        ItemSource::Fish { level, tool } => {
            // The tier's bait (feathers for trout and salmon, fishing bait
            // for sardines, herring and pike) is consumed one per catch
            // (`fishing.rs2` `inv_del(inv, $bait, 1)`): a real input, sized
            // with the quota like any other.
            let mut way = gather_way("fishing", "fishing", item, level, vec![tool]);
            way.inputs = fish_bait(item, level).map(|bait| vec![(bait.to_string(), 1)]).unwrap_or_default();
            way
        }
        ItemSource::Shear => {
            let mut way = Way::new(WayKind::Harvest, "shear sheep");
            way.tools = vec!["shears".to_string()];
            way.sites = npc_sites_by_name("sheepunsheered");
            way
        }
        ItemSource::Milk => {
            let mut way = Way::new(WayKind::Harvest, "milk a dairy cow");
            way.inputs = vec![("bucket_empty".to_string(), 1)];
            way
        }
        ItemSource::Pick { field } => {
            let mut way = Way::new(WayKind::Harvest, format!("pick {field}"));
            way.sites = station_sites(&field);
            way
        }
        ItemSource::Fill { point } => {
            let container = match item {
                "bucket_water" => "bucket_empty",
                "jug_water" => "jug_empty",
                "bowl_water" | "bowl_of_water" => "bowl_empty",
                "vial_water" => "vial_empty",
                _ => "",
            };
            let mut way = Way::new(WayKind::Harvest, format!("fill at a {point}"));
            if !container.is_empty() {
                way.inputs = vec![(container.to_string(), 1)];
            }
            way.sites = ["fountain", "sink", "well"].iter().flat_map(|kind| station_sites(kind)).collect();
            way
        }
        ItemSource::Tan { cost_gp } => {
            let mut way = Way::new(WayKind::Harvest, "tanner (Al-Kharid)");
            way.coins = cost_gp;
            way.inputs = vec![("cow_hide".to_string(), 1)];
            way.sites = npc_sites_by_name("tanner");
            way
        }
        ItemSource::Service { service } => {
            let npc = if service.starts_with("aggie") { "aggie" } else { "wyson_the_gardener" };
            // Aggie's dyes (`aggie.rs2`): 2 onions, 3 redberries or 2 woad
            // leaves and 5 coins per dye.
            let dye_input = match (service.as_str(), item) {
                ("aggie_dye", "yellowdye" | "yellow_dye") => Some(("onion", 2)),
                ("aggie_dye", "reddye" | "red_dye") => Some(("redberries", 3)),
                ("aggie_dye", "bluedye" | "blue_dye") => Some(("woadleaf", 2)),
                _ => None,
            };
            let mut way = Way::new(WayKind::Harvest, service);
            if let Some((input, count)) = dye_input {
                way.inputs = vec![(input.to_string(), count)];
                way.coins = 5;
            }
            way.sites = npc_sites_by_name(npc);
            way
        }
        ItemSource::DialogueBuy { npc, npc_id, cost, qty, file } => {
            let mut way = Way::new(WayKind::Dialogue, npc);
            way.file = file;
            way.coins = cost;
            way.qty = Some((qty, qty));
            way.sites = npc_sites(npc_id);
            way
        }
        ItemSource::Grant { trigger, subject, consumes, coins, quest, file } => {
            // A recipe handing back the container of one of its inputs
            // (the cake returns the milk's bucket) is a leftover, not a
            // way to make that container.
            let stem = item.split('_').next().unwrap_or(item);
            let returned = item.ends_with("_empty") && consumes.iter().any(|(input, _)| input != item && input.split('_').next() == Some(stem));
            let kind = if returned { WayKind::Leftover } else { WayKind::Script };
            let mut way = Way::new(kind, format!("[{trigger},{subject}]"));
            way.file = file;
            way.inputs = consumes;
            way.coins = coins;
            way.quest = quest;
            if trigger.starts_with("opnpc") {
                way.sites = npc_sites_by_name(&subject);
            } else if trigger.starts_with("oploc") {
                way.sites = crate::locs_by_name(&subject)
                    .iter()
                    .flat_map(|loc| loc.spawns.iter().filter(|spot| crate::free_zones::is_free(spot.x, spot.z)).map(|spot| (spot.x, spot.z, spot.level)))
                    .collect();
            }
            way
        }
        ItemSource::Spell { spell, input } => {
            let level = crate::spells::all_spells().into_iter().find(|row| row.spell == spell).map(|row| row.level).unwrap_or(1);
            // The cast burns its runes (`spells` table) with the object.
            let runes: Vec<(String, i64)> = crate::spells::all_spells()
                .into_iter()
                .find(|row| row.spell == spell)
                .map(|row| row.runes.iter().map(|(rune, count)| (rune.to_string(), *count)).collect())
                .unwrap_or_default();
            let mut way = Way::new(WayKind::Spell, spell);
            way.skill = Some(("magic".to_string(), level));
            way.inputs = vec![(input, 1)];
            way.inputs.extend(runes);
            way
        }
        ItemSource::Mix { kind, inputs } => {
            let mut way = Way::new(WayKind::Mix, kind);
            way.inputs = inputs.into_iter().map(|input| (input, 1)).collect();
            way
        }
        ItemSource::Leftover { from } => {
            let mut way = Way::new(WayKind::Leftover, format!("after consuming {from}"));
            way.inputs = vec![(from, 1)];
            way
        }
    };
    Some(way)
}

/// Bait one catch of `item` burns at the tier opening at `level`.
fn fish_bait(item: &str, level: i64) -> Option<&'static str> {
    crate::fishing::all_fish_tiers()
        .into_iter()
        .find(|tier| tier.level == level && tier.products.contains(&item))
        .and_then(|tier| tier.bait)
}

/// F2P quest a gather way needs (server content, rev 274): rune
/// essence is mined only past Aubury's teleport, which
/// `skill_runecraft/scripts/essence_mine.rs2` refuses while
/// `%runemysteries < ^runemysteries_complete` (Rune Mysteries).
pub fn gather_quest(item: &str) -> Option<&'static str> {
    match item {
        "blankrune" => Some("quest_runemysteries"),
        _ => None,
    }
}

fn gather_way(skill: &str, site_kind: &str, item: &str, level: i64, tools: Vec<String>) -> Way {
    let mut way = Way::new(WayKind::Gather, format!("{skill} {item}"));
    way.skill = Some((skill.to_string(), level.max(1)));
    way.tools = tools;
    way.sites = crate::danger::sites(site_kind, item);
    way
}

/// True when some F2P way yields `item`, every input of that way being
/// itself obtainable (fixed point over the F2P objects: cycle-safe).
pub fn obtainable_f2p(item: &str) -> bool {
    index().obtainable.contains(item)
}

/// D-A: a player can make or gather `item` in F2P — some production way
/// (recipe, skill harvest, mixing, spell, item-on-item script) whose
/// every input is itself obtainable in F2P.
pub fn producible_f2p(item: &str) -> bool {
    ways(item).iter().any(|way| {
        way.is_production() && way.inputs.iter().all(|(input, _)| input != item && obtainable_f2p(input))
    })
}

/// D-A: NPC-shop purchases are reserved for F2P objects nobody can make,
/// gather or farm in F2P (shears, nets, rods…). Producible objects and
/// drops a bot can farm (feathers from chickens, bait from men: user
/// rule 2026-11-13) come from players: the market, or the bot's own work.
pub fn merchant_buy_allowed(item: &str) -> bool {
    crate::objs_by_name(item).into_iter().next().is_some_and(|obj| obj.members == Some(false))
        && !producible_f2p(item)
        && crate::combat_spots::drop_spots(item).is_empty()
}

/// Why an F2P tradeable object has no F2P way at all (rev 274 truth):
/// `None` when some way exists.
pub fn unobtainable_reason(item: &str) -> Option<&'static str> {
    if !ways(item).is_empty() {
        return None;
    }
    let reason = if ["_trim", "_gold", "_guthix", "_saradomin", "_zamorak"].iter().any(|suffix| item.ends_with(suffix)) {
        "treasure trail reward (clue scrolls are members in rev 274)"
    } else if ["partyhat", "santa_hat", "halloweenmask", "easter_egg", "christmas_cracker", "pumpkin"].iter().any(|mark| item.contains(mark)) {
        "holiday event item"
    } else if item.starts_with("macro_") {
        "random event item"
    } else if item.starts_with("newbie") {
        "Tutorial Island item"
    } else if ["tbwt_", "mm_", "chompy", "viking_", "wolfengloves_", "discofreturning", "farmers_fork", "highwayman_mask"]
        .iter()
        .any(|mark| item.contains(mark))
    {
        "members quest or minigame item"
    } else {
        "no F2P source in rev 274 (members-only shop, drop or recipe)"
    };
    Some(reason)
}

#[cfg(test)]
#[path = "obtain_tests.rs"]
mod tests;
