//! Supply indices (dashboard `/items`, user 2026-11-18): eleven families
//! of objects whose fleet holdings sum into one curve each, to read the
//! "supply" at a glance. Every family is derived from the dumps, never
//! from a hand list; the few hand rules left (containers out of the
//! cooking chain, consumed thread out of the tools) are named below and
//! pinned by `supply_indices_tests`.
//!
//! Universe: the fleet is F2P (D4), so a member is an object with
//! `members == Some(false)` and `tradeable`, never a banknote. A note
//! (`cert_<name>`) counts toward its base object: `obj_ids` lists both.

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use crate::equipment::{all_wearables, Wearable};
use crate::gear::{self, AMMO, WEAPON};

/// One supply index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SupplyIndex {
    Cooking,
    Food,
    MeleeEquipment,
    RangeEquipment,
    RangeSupply,
    MagicEquipment,
    MagicSupply,
    Logs,
    Tools,
    MiningSupply,
    SmithingSupply,
}

impl SupplyIndex {
    /// Every index, in display order.
    pub const ALL: [SupplyIndex; 11] = [
        SupplyIndex::Cooking,
        SupplyIndex::Food,
        SupplyIndex::MeleeEquipment,
        SupplyIndex::RangeEquipment,
        SupplyIndex::RangeSupply,
        SupplyIndex::MagicEquipment,
        SupplyIndex::MagicSupply,
        SupplyIndex::Logs,
        SupplyIndex::Tools,
        SupplyIndex::MiningSupply,
        SupplyIndex::SmithingSupply,
    ];

    /// Stable key (API `index=` value).
    pub fn key(self) -> &'static str {
        match self {
            SupplyIndex::Cooking => "cooking",
            SupplyIndex::Food => "food",
            SupplyIndex::MeleeEquipment => "melee_equipment",
            SupplyIndex::RangeEquipment => "range_equipment",
            SupplyIndex::RangeSupply => "range_supply",
            SupplyIndex::MagicEquipment => "magic_equipment",
            SupplyIndex::MagicSupply => "magic_supply",
            SupplyIndex::Logs => "logs",
            SupplyIndex::Tools => "tools",
            SupplyIndex::MiningSupply => "mining_supply",
            SupplyIndex::SmithingSupply => "smithing_supply",
        }
    }

    /// Display label (dashboard).
    pub fn label(self) -> &'static str {
        match self {
            SupplyIndex::Cooking => "Cooking",
            SupplyIndex::Food => "Food",
            SupplyIndex::MeleeEquipment => "Melee equipment",
            SupplyIndex::RangeEquipment => "Range equipment",
            SupplyIndex::RangeSupply => "Range supply",
            SupplyIndex::MagicEquipment => "Magic equipment",
            SupplyIndex::MagicSupply => "Magic supply",
            SupplyIndex::Logs => "Logs",
            SupplyIndex::Tools => "Tools",
            SupplyIndex::MiningSupply => "Mining supply",
            SupplyIndex::SmithingSupply => "Smithing supply",
        }
    }

    /// The index of a key, `None` when unknown.
    pub fn parse(key: &str) -> Option<SupplyIndex> {
        SupplyIndex::ALL.into_iter().find(|index| index.key() == key)
    }
}

/// Filled containers and utensils met along the cooking prep chain: the
/// vessel, not the food (`pot_empty` holds the flour, the pie dish and
/// the cake tin come back after baking, `leftover` rows of
/// `config_recipes`). Water is filled for free at any sink and shared
/// with pottery (soft clay), and so is the stew's water bowl: not a
/// cooking ingredient either.
pub const COOKING_VESSELS: [&str; 5] = ["pot_empty", "piedish", "cake_tin", "jug_water", "bowl_water"];

/// Stew ingredients the dump cannot see: `stew.rs2` reads them from the
/// used objects at run time (`make_incomplete_stew`: water bowl plus
/// potato or meat; `make_uncooked_stew`: the other one), so the script
/// way of `uncooked_stew` lists no input. The two incomplete stews and
/// the potato are the non-edible links of that chain.
pub const STEW_SCRIPT_INGREDIENTS: [&str; 3] = ["potato", "stew1", "stew2"];

/// Tool rows the server consumes: thread is used up as leather is sewn
/// (`leather.rs2`, one spool per five items), so it is a supply, never a
/// tool, although `obtain` carries it beside the needle.
pub const CONSUMED_TOOL_INPUTS: [&str; 1] = ["thread"];

/// True for an object of the F2P market universe (members flag false,
/// tradeable, not a banknote).
pub fn in_universe(name: &str) -> bool {
    if name.starts_with("cert_") {
        return false;
    }
    crate::objs_by_name(name).into_iter().any(|obj| obj.members == Some(false) && obj.tradeable)
}

/// Base debug names of an index, sorted.
pub fn members(index: SupplyIndex) -> &'static [String] {
    static TABLE: OnceLock<HashMap<SupplyIndex, Vec<String>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| SupplyIndex::ALL.into_iter().map(|index| (index, compute_members(index))).collect());
    table.get(&index).map(Vec::as_slice).unwrap_or(&[])
}

/// Object ids that count toward an index: every base member and its
/// banknote, sorted.
pub fn obj_ids(index: SupplyIndex) -> Vec<usize> {
    let mut ids = BTreeSet::new();
    for name in members(index) {
        for candidate in [name.clone(), format!("cert_{name}")] {
            for obj in crate::objs_by_name(&candidate) {
                ids.insert(obj.obj_id);
            }
        }
    }
    ids.into_iter().collect()
}

/// Indices an object id counts toward (its banknote folds on the base).
pub fn indices_of(obj_id: usize) -> Vec<SupplyIndex> {
    let Some(name) = crate::obj_by_id(obj_id).and_then(|obj| obj.name) else { return Vec::new() };
    let base = name.strip_prefix("cert_").unwrap_or(name);
    SupplyIndex::ALL.into_iter().filter(|index| members(*index).iter().any(|member| member == base)).collect()
}

fn compute_members(index: SupplyIndex) -> Vec<String> {
    let raw: BTreeSet<String> = match index {
        SupplyIndex::Cooking => cooking_ingredients(),
        SupplyIndex::Food => crate::food::all_foods().into_iter().map(|(name, _)| name).collect(),
        SupplyIndex::MeleeEquipment => wearables_of(Some(gear::Style::Melee)),
        SupplyIndex::RangeEquipment => wearables_of(Some(gear::Style::Ranged)),
        SupplyIndex::MagicEquipment => wearables_of(Some(gear::Style::Magic)),
        SupplyIndex::RangeSupply => ammunition(),
        SupplyIndex::MagicSupply => runes(),
        SupplyIndex::Logs => crate::trees::all_tiers().into_iter().map(|tier| tier.product).collect(),
        SupplyIndex::Tools => tools(),
        SupplyIndex::MiningSupply => crate::rocks::all_rock_tiers().into_iter().map(|tier| tier.product).collect(),
        SupplyIndex::SmithingSupply => smelted_bars(),
    };
    raw.into_iter().filter(|name| in_universe(name)).collect()
}

fn is_food(name: &str) -> bool {
    crate::food::heal(name) > 0
}

/// Cooking: every input of an F2P cook row that pays XP (the burn rows
/// `cooked_meat -> burnt_meat` pay none), then up the prep chain (cook
/// prep and mill rows, item-on-item scripts that make a cook input) to
/// the raw ingredients (flour, grain, egg, milk, berries…). Edible
/// objects stay food (cooked meat in a pie, tomato and cheese on a
/// pizza) and vessels stay out: the indices never count one object twice.
fn cooking_ingredients() -> BTreeSet<String> {
    let mut frontier: Vec<String> = crate::recipes::all_recipes(Some("cook"))
        .into_iter()
        .filter(|recipe| recipe.members == Some(false) && recipe.xp > 0)
        .flat_map(|recipe| recipe.inputs.into_iter().map(|(name, _)| name))
        .collect();
    frontier.extend(STEW_SCRIPT_INGREDIENTS.iter().map(|name| name.to_string()));
    let mut found = BTreeSet::new();
    while let Some(name) = frontier.pop() {
        if COOKING_VESSELS.contains(&name.as_str()) || is_food(&name) || !found.insert(name.clone()) {
            continue;
        }
        frontier.extend(prep_inputs(&name));
    }
    found
}

/// Consumed inputs of the ways that prepare a cooking ingredient: the
/// `cook_prep`/`mill` rows and production scripts (uncooked stew).
fn prep_inputs(name: &str) -> Vec<String> {
    let mut inputs: Vec<String> = crate::extra_recipes::all_extra()
        .into_iter()
        .filter(|row| (row.kind == "cook_prep" || row.kind == "mill") && row.outputs.iter().any(|(output, _)| output == name))
        .flat_map(|row| row.inputs.into_iter().map(|(input, _)| input))
        .collect();
    for way in crate::obtain::ways(name) {
        if way.kind == crate::obtain::WayKind::Script && way.is_production() {
            inputs.extend(way.inputs.into_iter().map(|(input, _)| input));
        }
    }
    inputs
}

fn melee_attack(entry: &Wearable) -> i64 {
    ["stab", "slash", "crush"].iter().map(|key| bonus(&entry.attack, key)).max().unwrap_or(0)
}

fn bonus(pairs: &[(String, i64)], key: &str) -> i64 {
    pairs.iter().find(|(name, _)| name == key).map(|(_, value)| *value).unwrap_or(0)
}

fn requires(entry: &Wearable, skill: &str) -> bool {
    entry.requirements.iter().any(|(name, _)| name == skill)
}

/// Combat style of a wearable for the indices, `None` for no style
/// (cosmetics, unenchanted jewellery, capes, tools, ammunition).
///
/// Weapons: harvest tools (pickaxes, woodcutting axes) are tools; bows
/// and crossbows ranged; staves magic; anything else melee when it has a
/// melee attack or strength bonus (the zero-stat broken-tool props stay
/// out). Ammunition is range supply, never equipment. Armour and
/// jewellery: the strongest offensive bonus wins (melee = best of
/// stab/slash/crush or strength), ties to melee; without any, the wield
/// requirement decides (ranged: leather gloves and boots; attack,
/// strength, defence: melee), then an `armour_*` category (wooden
/// shield) or a defence bonus against every style (amulet of defence,
/// holy symbol) reads melee.
pub fn wearable_style(entry: &Wearable) -> Option<gear::Style> {
    if entry.wearpos == AMMO {
        return None;
    }
    if entry.wearpos == WEAPON {
        if gear::is_tool(entry) {
            return None;
        }
        if gear::is_ranged_weapon(entry) {
            return Some(gear::Style::Ranged);
        }
        if gear::is_magic_weapon(entry) {
            return Some(gear::Style::Magic);
        }
        return (melee_attack(entry) > 0 || entry.strength > 0).then_some(gear::Style::Melee);
    }
    let melee = melee_attack(entry).max(entry.strength);
    let range = bonus(&entry.attack, "range");
    let magic = bonus(&entry.attack, "magic");
    if melee.max(range).max(magic) > 0 {
        return Some(if melee >= range && melee >= magic {
            gear::Style::Melee
        } else if range >= magic {
            gear::Style::Ranged
        } else {
            gear::Style::Magic
        });
    }
    if requires(entry, "ranged") {
        return Some(gear::Style::Ranged);
    }
    if requires(entry, "magic") {
        return Some(gear::Style::Magic);
    }
    if ["attack", "strength", "defence"].iter().any(|skill| requires(entry, skill)) {
        return Some(gear::Style::Melee);
    }
    let all_round = ["stab", "slash", "crush", "magic", "range"].iter().all(|key| bonus(&entry.defence, key) > 0);
    if entry.category.as_deref().is_some_and(|category| category.starts_with("armour_")) || all_round {
        return Some(gear::Style::Melee);
    }
    None
}

fn wearables_of(style: Option<gear::Style>) -> BTreeSet<String> {
    all_wearables().into_iter().filter(|entry| wearable_style(entry) == style).map(|entry| entry.name).collect()
}

/// Range supply: quiver ammunition with a ranged strength (arrows,
/// bolts; the server reads `rangebonus` when the shot leaves).
fn ammunition() -> BTreeSet<String> {
    all_wearables().into_iter().filter(|entry| entry.wearpos == AMMO && entry.rangebonus > 0).map(|entry| entry.name).collect()
}

/// Magic supply: every rune an F2P spell burns, plus what runecrafting
/// makes.
fn runes() -> BTreeSet<String> {
    let mut found: BTreeSet<String> = crate::spells::all_spells()
        .into_iter()
        .filter(|spell| !spell.members)
        .flat_map(|spell| spell.runes.into_iter().map(|(rune, _)| rune))
        .collect();
    for row in crate::extra_recipes::by_kind("runecraft") {
        found.extend(row.outputs.into_iter().map(|(rune, _)| rune));
    }
    found
}

/// Smithing supply: every bar the furnace smelts (`smelt` rows; gold and
/// silver bars included, they are smelted for Smithing XP too).
fn smelted_bars() -> BTreeSet<String> {
    crate::recipes::all_recipes(Some("smelt")).into_iter().flat_map(|recipe| recipe.outputs.into_iter().map(|(name, _)| name)).collect()
}

/// Tools: everything a way carries without consuming it (`obtain`
/// tools: hammer, chisel, needle, tinderbox, shears, moulds, talismans),
/// the fishing gear of every tier, and the harvest tools worn in hand
/// (pickaxes, woodcutting axes; the random-event broken pickaxes have
/// no stat at all and mine nothing: props, not tools).
fn tools() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for obj in crate::f2p::f2p_objs() {
        let Some(name) = obj.name else { continue };
        for way in crate::obtain::ways(name) {
            found.extend(way.tools);
        }
    }
    for tier in crate::fishing::all_fish_tiers() {
        found.insert(tier.tool.to_string());
    }
    for entry in all_wearables() {
        if entry.wearpos == WEAPON && gear::is_tool(&entry) && melee_attack(&entry) > 0 {
            found.insert(entry.name);
        }
    }
    for consumed in CONSUMED_TOOL_INPUTS {
        found.remove(consumed);
    }
    found
}

#[cfg(test)]
#[path = "supply_indices_tests.rs"]
mod supply_indices_tests;
