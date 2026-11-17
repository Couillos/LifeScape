//! Every F2P activity the guides list (plan rendements R4, §2: "une
//! ligne pour CHAQUE méthode jouable en F2P 274"), derived from the game
//! data and the planner, never a hand list:
//! - every production leg of every F2P object (`obtain_legs`: gathers,
//!   drop farms, recipes, pickups, harvests, enchants, superheat,
//!   telegrab), keyed by its ACTIVITY (`activity`: a drop farm is its
//!   fight, a fishing leg its spot option);
//! - the fights: each melee rung in each melee style, the ranged and
//!   magic spots the training tranches use (the best spell of the band);
//! - every D18 spell method (teleports, curses, bananas, crumble
//!   undead), firemaking of every log, burying every bone, the essence
//!   trips, every method a training tranche launches (`reach_method`);
//! - alchemy of every object another guide method makes (a smith's or
//!   a jeweller's outlet).
//!
//! Each method carries its band skill and skill gate, and per band the
//! tool tier a player of that level works with.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, OnceLock};

use lifescape_gamedata::obtain::WayKind;

/// One activity of the guides.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GuideMethod {
    pub method: String,
    pub style: String,
}

/// Melee styles a melee fight is published in (each trains its skill).
pub const MELEE_STYLES: [&str; 4] = ["melee:attack", "melee:strength", "melee:defence", "melee:controlled"];
/// The melee ladder (a rung chosen at run time, never an activity).
pub const LADDER: &str = "fight:melee/ladder";
/// Magic level of low alchemy, the first alchemy spell (`alchemy`).
pub const LOW_ALCHEMY_LEVEL: u8 = 21;
/// F2P skills (the training guide's skills).
pub const SKILLS: [&str; 15] = ["attack", "strength", "defence", "hitpoints", "ranged", "prayer", "magic", "cooking", "woodcutting", "fishing", "firemaking", "crafting", "smithing", "mining", "runecraft"];

fn trading(method: &str) -> bool {
    ["market:", "shop:", "dialogue:"].iter().any(|prefix| method.starts_with(prefix))
}

fn push(out: &mut Vec<GuideMethod>, method: String) {
    if method.starts_with("fight:melee/") {
        for style in MELEE_STYLES {
            out.push(GuideMethod { method: method.clone(), style: style.to_string() });
        }
        return;
    }
    let style = if method.starts_with("fight:ranged/") {
        "ranged"
    } else if method.starts_with("fight:magic/") {
        "magic"
    } else {
        ""
    };
    out.push(GuideMethod { method, style: style.to_string() });
}

fn compute_catalogue() -> Vec<GuideMethod> {
    let mut methods: Vec<String> = Vec::new();
    let mut made: Vec<String> = Vec::new();
    for obj in lifescape_gamedata::all_objs() {
        let Some(name) = obj.name else { continue };
        if obj.members != Some(false) || name.starts_with("cert_") {
            continue;
        }
        for leg in crate::obtain_legs::production_legs(name) {
            if trading(&leg.method) {
                continue;
            }
            if ["smith:", "jewelry:", "string:", "craft:", "enchant:"].iter().any(|prefix| leg.method.starts_with(prefix)) {
                made.push(name.to_string());
            }
            methods.push(crate::activity::activity_of_method(&leg.method));
        }
    }
    for rung in lifescape_gamedata::melee_ladder::RUNGS {
        methods.push(format!("fight:melee/{}", rung.key));
    }
    for level in (1..=99u8).step_by(5) {
        methods.push(format!("fight:ranged/{}", crate::tranches::tranche_resource("ranged", level)));
        let spot = crate::tranches::tranche_resource("magic", level);
        methods.push(format!("fight:magic/{}/{spot}", crate::method_way::best_combat_spell_at(level, &spot)));
        for skill in SKILLS {
            methods.push(crate::activity::activity_of_method(&crate::tranches::reach_method(skill, level)));
        }
    }
    methods.extend(crate::spell_methods::all_spell_methods());
    for recipe in lifescape_gamedata::extra_recipes::by_kind("firemaking") {
        // The recipe lists the tinderbox beside the log: only a log burns.
        methods.extend(recipe.inputs.iter().map(|(log, _)| format!("fire:{log}")).filter(|method| crate::method_way::method_way(method).is_some()));
    }
    for recipe in lifescape_gamedata::extra_recipes::by_kind("bury") {
        methods.extend(recipe.inputs.iter().map(|(bones, _)| format!("bury:{bones}")));
    }
    methods.extend(made.iter().filter(|item| crate::alchemy::alch_value(item, true).is_some()).map(|item| format!("alch:{item}")));
    // The melee ladder picks its rung at run time: each rung is listed.
    methods.retain(|method| method != LADDER);
    methods.sort();
    methods.dedup();
    let mut out = Vec::new();
    for method in methods {
        push(&mut out, method);
    }
    out.sort();
    out.dedup();
    out
}

/// Every activity of the guides (memoised: static game data).
pub fn catalogue() -> &'static [GuideMethod] {
    static CATALOGUE: OnceLock<Vec<GuideMethod>> = OnceLock::new();
    CATALOGUE.get_or_init(compute_catalogue)
}

/// Process memo of a pure function of the static game data.
pub fn memo<K: Eq + Hash + Clone, V: Clone>(table: &'static OnceLock<Mutex<HashMap<K, V>>>, key: K, compute: impl FnOnce() -> V) -> V {
    let map = table.get_or_init(Default::default);
    if let Some(found) = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        return found.clone();
    }
    let value = compute();
    map.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key, value.clone());
    value
}

/// Skill gate of a method: the level its way asks in its skill (a
/// combat spell its spell level, low alchemy 21), 1 without one.
pub fn gate_level(method: &str) -> u8 {
    static MEMO: OnceLock<Mutex<HashMap<String, u8>>> = OnceLock::new();
    memo(&MEMO, method.to_string(), || compute_gate_level(method))
}

fn compute_gate_level(method: &str) -> u8 {
    if let Some(spell) = crate::method_way::magic_spell(method) {
        let level = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell).map(|row| row.level).unwrap_or(1);
        return level.clamp(1, 99) as u8;
    }
    if method.starts_with("alch:") {
        return LOW_ALCHEMY_LEVEL;
    }
    crate::method_way::method_way(method)
        .or_else(|| crate::spell_methods::spell_way(method))
        .and_then(|(_, way)| way.skill.map(|(_, level)| level))
        .unwrap_or(1)
        .clamp(1, 99) as u8
}

/// Tool tier a player at `level` works `method` with (prior rows): the
/// best pickaxe or axe the level opens, the best melee weapon of a melee
/// fighter at that level in every melee stat; empty otherwise.
pub fn tool_at(method: &str, style: &str, level: u8) -> String {
    compute_tool_at(method, style, level)
}

/// Which tool tier a method works with (`tool_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ToolKind {
    Pickaxe,
    Axe,
    MeleeWeapon,
    Nothing,
}

fn compute_tool_at(method: &str, style: &str, level: u8) -> String {
    let kind = if method.starts_with("gather:mining/") || method.starts_with("essence:") {
        ToolKind::Pickaxe
    } else if method.starts_with("gather:woodcut/") {
        ToolKind::Axe
    } else if method.starts_with("fight:melee/") || style.starts_with("melee") {
        ToolKind::MeleeWeapon
    } else {
        ToolKind::Nothing
    };
    // The tier depends on the kind and the level only: the gear search
    // runs once per (kind, level), never per method.
    static MEMO: OnceLock<Mutex<HashMap<(ToolKind, u8), String>>> = OnceLock::new();
    memo(&MEMO, (kind, level), || tool_of_kind(kind, level))
}

fn tool_of_kind(kind: ToolKind, level: u8) -> String {
    let level = i64::from(level.max(1));
    if kind == ToolKind::Pickaxe {
        return lifescape_gamedata::gear::best_tool("pickaxe", &[("mining", level)]).unwrap_or_default();
    }
    if kind == ToolKind::Axe {
        return lifescape_gamedata::gear::best_tool("axe", &[("woodcutting", level)]).unwrap_or_default();
    }
    if kind == ToolKind::MeleeWeapon {
        let levels = [("attack", level), ("strength", level), ("defence", level)];
        return lifescape_gamedata::gear::best_gear(lifescape_gamedata::gear::Style::Melee, &levels, &[])
            .into_iter()
            .find(|pick| pick.slot == lifescape_gamedata::gear::WEAPON)
            .map(|pick| pick.item)
            .unwrap_or_default();
    }
    String::new()
}

/// Product an activity exists for (its way's object), if any.
pub fn own_product(method: &str) -> Option<String> {
    static MEMO: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    memo(&MEMO, method.to_string(), || compute_own_product(method))
}

fn compute_own_product(method: &str) -> Option<String> {
    crate::method_way::method_way(method).or_else(|| crate::spell_methods::spell_way(method)).map(|(product, _)| product).filter(|product| !product.is_empty())
}

/// True when the method's work transforms consumed inputs (a recipe,
/// alchemy, burning, burying, runecraft), false for a harvest or fight
/// (its consumables are not stock).
pub fn transforms(method: &str) -> bool {
    if method.starts_with("alch:") {
        return true;
    }
    match crate::method_way::method_way(method) {
        Some((_, way)) => matches!(way.kind, WayKind::Make | WayKind::Spell) && !way.inputs.is_empty() && !method.starts_with("ground:"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{catalogue, gate_level, tool_at};

    /// The catalogue holds every activity family, keyed by activity
    /// (no `farm:` drop or fishing alias left), each melee fight in the
    /// four styles.
    #[test]
    fn catalogue_covers_every_family_by_activity() {
        let methods = catalogue();
        let has = |method: &str| methods.iter().any(|entry| entry.method == method);
        for method in ["gather:mining/coal", "gather:woodcut/yew_logs", "gather:fishing/raw_trout", "smelt:steel_bar", "smith:iron_platebody", "cook:swordfish", "fire:logs", "bury:bones", "runecraft:airrune", "essence:blankrune", "ground:airrune", "teleport:varrock_teleport", "bananas:bones", "alch:iron_platebody", "fight:melee/chickens"] {
            assert!(has(method), "{method} listed");
        }
        assert!(!methods.iter().any(|entry| entry.method.starts_with("farm:")), "farm legs keyed by activity");
        assert!(!has("gather:fishing/raw_salmon"), "salmon fishes the trout option");
        let chickens: Vec<&str> = methods.iter().filter(|entry| entry.method == "fight:melee/chickens").map(|entry| entry.style.as_str()).collect();
        assert_eq!(chickens.len(), 4, "{chickens:?}");
        assert!(methods.iter().any(|entry| entry.method.starts_with("curse:")));
        assert!(methods.iter().any(|entry| entry.method.starts_with("fight:magic/") && entry.style == "magic"));
    }

    #[test]
    fn gates_and_tools_follow_the_level() {
        assert_eq!(gate_level("smelt:steel_bar"), 30);
        assert_eq!(gate_level("fight:magic/fire_strike/cows"), 13);
        assert_eq!(gate_level("alch:iron_platebody"), 21);
        assert_eq!(tool_at("gather:mining/coal", "", 41), "rune_pickaxe");
        assert_eq!(tool_at("gather:mining/coal", "", 1), "iron_pickaxe");
        assert_eq!(tool_at("smelt:steel_bar", "", 40), "");
    }
}
