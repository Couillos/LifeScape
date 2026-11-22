//! Method → game-data way: which `gamedata::obtain::Way` a planner
//! method works (its product, skill gate, consumed inputs, carried tools,
//! quest and sites). One derivation for every consumer: the tool gate
//! (`tools::required_tool`), the production legs (`obtain_legs`), the
//! tranche sizing (`sizing`) and the skeleton dependencies (`expand`).
//! The gear lives in the server data (`Way.tools`), never in a hand list.

use lifescape_gamedata::obtain::{ways, Way, WayKind};

/// Combat spell a magic method casts: `fight:magic/<spell>/<spot>`, or
/// the legacy `fight:magic/<spot>` (wind strike, the level-1 spell).
pub fn magic_spell(method: &str) -> Option<String> {
    let tail = method.strip_prefix("fight:magic/")?;
    match tail.split_once('/') {
        Some((spell, _)) => Some(spell.to_string()),
        None => Some("wind_strike".to_string()),
    }
}

/// Spot of a fight method (`fight:<style>/<spot>`, magic with its spell).
pub fn fight_spot(method: &str) -> Option<&str> {
    let tail = method.strip_prefix("fight:")?;
    Some(tail.rsplit('/').next().unwrap_or(tail))
}

/// Best F2P combat spell castable at `level` on any target (strike,
/// bolt, blast): a damage spell of the dump (`Spell::deals_damage`, never
/// its display name: the server gives crumble undead none) whose target
/// needs no param (crumble undead hits the undead only).
pub fn best_combat_spell(level: u8) -> String {
    best_combat_spell_where(level, |spell| lifescape_gamedata::spells::target_param(spell).is_none())
}

/// Best F2P combat spell castable at `level` on every NPC of the combat
/// spot `spot`: crumble undead (39) on an all-undead spot, the elemental
/// ladder elsewhere (`spell_methods::spell_fits_spot`, server params).
pub fn best_combat_spell_at(level: u8, spot: &str) -> String {
    best_combat_spell_where(level, |spell| crate::spell_methods::spell_fits_spot(spell, spot))
}

fn best_combat_spell_where(level: u8, fits: impl Fn(&str) -> bool) -> String {
    lifescape_gamedata::spells::all_spells()
        .into_iter()
        .filter(|spell| !spell.members && spell.deals_damage() && spell.level <= i64::from(level) && fits(&spell.spell))
        .max_by_key(|spell| spell.level)
        .map(|spell| spell.spell.to_string())
        .unwrap_or_else(|| "wind_strike".to_string())
}

/// Runes one cast of `spell` burns (server `spells` table).
pub fn spell_runes(spell: &str) -> Vec<(String, i64)> {
    lifescape_gamedata::spells::all_spells()
        .into_iter()
        .find(|row| row.spell == spell)
        .map(|row| row.runes.iter().map(|(rune, count)| (rune.to_string(), *count)).collect())
        .unwrap_or_default()
}

/// True for an uncooked stage the meals scripts assemble in the pack
/// (bread dough, pie shells, uncooked pies, pizza, cake, stew, wine).
pub fn prepared(input: &str) -> bool {
    input.starts_with("uncooked_")
        || input == "jug_unfermented_wine"
        || ways(input).iter().any(|way| way.kind == WayKind::Make && way.detail.starts_with("cook_prep"))
}

/// Inputs with every prepared stage replaced by its own inputs (the
/// base components a meals script banks: flour, water, pie dish,
/// berries...). Bounded depth: the prep chain is three stages deep.
pub fn flatten_prep(inputs: &[(String, i64)]) -> Vec<(String, i64)> {
    fn expand(item: &str, count: i64, depth: u8, out: &mut Vec<(String, i64)>) {
        let prep = ways(item).into_iter().find(|way| way.kind == WayKind::Make && way.detail.starts_with("cook_prep"));
        match prep {
            Some(way) if depth > 0 => {
                for (input, per) in &way.inputs {
                    expand(input, count * (*per).max(1), depth - 1, out);
                }
            }
            _ => match out.iter_mut().find(|(name, _)| name == item) {
                Some(known) => known.1 += count,
                None => out.push((item.to_string(), count)),
            },
        }
    }
    let mut out = Vec::new();
    for (input, count) in inputs {
        expand(input, *count, 4, &mut out);
    }
    out
}

/// True when a component comes back once its stage is done: the server
/// leaves it behind (`config_recipes` leftover, `next_obj_stage`: an
/// uncooked cake leaves its cake tin). A carried tool, never consumed
/// per unit (live admin 2026-11-15: `bake:cake` priced and staged
/// without its tin, the bake blocked on it).
fn returned_by(stages: &[String], component: &str) -> bool {
    lifescape_gamedata::script_sources::config_recipes()
        .iter()
        .any(|row| row.kind == "leftover" && row.output == component && row.inputs.iter().any(|input| stages.contains(input)))
}

/// A spawn tile a bot may sweep: free, behind no gate but the Karamja
/// boat (a fare, user 2026-11-16), with a bank in reach some combat level
/// makes safe (the Wilderness included).
pub(crate) fn spawn_workable(site: &(i64, i64, i64)) -> bool {
    let (x, z, level) = *site;
    lifescape_gamedata::free_zones::is_free(x, z)
        && lifescape_gamedata::access::gates_at(x, z, level).iter().all(lifescape_gamedata::access::fare_only)
        && lifescape_gamedata::danger::site_requirement(x, z, level) < lifescape_gamedata::danger::NEVER
}

/// A ground spawn a bot may sweep (one of its tiles workable).
pub fn ground_workable(way: &Way) -> bool {
    way.kind == WayKind::Ground && way.sites.iter().any(spawn_workable)
}

/// Superheat (`superheat_item`, magic 43): the smelt inputs plus the
/// spell's runes, at any spot, never failing (no iron loss).
fn superheat_way(bar: &str) -> Option<Way> {
    let smelt = make_way_of(bar, &["smelt"])?;
    let spell = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == "superheat_item")?;
    let mut way = smelt;
    way.kind = WayKind::Spell;
    way.detail = format!("superheat_item {bar}");
    way.skill = Some(("magic".to_string(), spell.level));
    way.inputs.extend(spell.runes.iter().map(|(rune, count)| (rune.to_string(), *count)));
    way.sites = Vec::new();
    way.success = 1.0;
    Some(way)
}

/// Telekinetic grab of a ground spawn (wine of Zamorak: the monks attack
/// a hand pickup): magic 33, one law and one air rune per grab.
fn telegrab_way(item: &str) -> Option<Way> {
    let mut way = ways(item).into_iter().filter(|way| way.kind == WayKind::Ground).reduce(|mut merged, next| {
        merged.sites.extend(next.sites);
        merged
    })?;
    let spell = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == "telekinetic_grab")?;
    way.kind = WayKind::Spell;
    way.detail = format!("telekinetic_grab {item}");
    way.skill = Some(("magic".to_string(), spell.level));
    way.inputs = spell.runes.iter().map(|(rune, count)| (rune.to_string(), *count)).collect();
    way.sites.retain(spawn_workable);
    Some(way)
}

/// Every way of `item` a planner leg can work: the obtain index plus the
/// ways the index has no row for (superheat of a smeltable bar, the grab
/// of the wine of Zamorak).
pub fn production_ways(item: &str) -> Vec<Way> {
    let mut out: Vec<Way> = ways(item);
    if item == "wine_of_zamorak" {
        out.retain(|way| way.kind != WayKind::Ground);
        out.extend(telegrab_way(item));
    }
    out.extend(superheat_way(item));
    out
}

fn make_way_of(product: &str, kinds: &[&str]) -> Option<Way> {
    ways(product)
        .into_iter()
        .filter(|way| way.kind == WayKind::Make && kinds.contains(&way.detail.split_whitespace().next().unwrap_or("")))
        .min_by_key(|way| way.skill.as_ref().map(|(_, level)| *level).unwrap_or(0))
}

fn gather_way_of(item: &str) -> Option<Way> {
    ways(item).into_iter().filter(|way| way.kind == WayKind::Gather).min_by_key(|way| way.skill.as_ref().map(|(_, level)| *level).unwrap_or(0))
}

/// Make way consuming `input` among `kind` recipes (stringing, burning).
fn make_way_using(input: &str, kind: &str) -> Option<(String, Way)> {
    let recipe = lifescape_gamedata::extra_recipes::by_kind(kind).into_iter().find(|row| row.inputs.iter().any(|(name, _)| name == input))?;
    let product = recipe.outputs.first().map(|(name, _)| name.clone()).unwrap_or_else(|| "ashes".to_string());
    let way = ways(&product)
        .into_iter()
        .find(|way| way.kind == WayKind::Make && way.inputs.iter().any(|(name, _)| name == input) && way.detail.starts_with(kind))?;
    Some((product, way))
}

/// Product a raw object cooks into (`recipes` cook rows).
pub fn cooked_of(raw: &str) -> Option<String> {
    lifescape_gamedata::recipes::using(raw, Some("cook")).into_iter().find_map(|recipe| recipe.outputs.first().map(|(name, _)| name.clone()))
}

/// The way one method works and the object it yields. `None` for
/// methods with no game-data way (fights, purchases, quests, walks).
/// Memoised for the process: a pure function of the static game data
/// (the guides ask it thousands of times a build).
pub fn method_way(method: &str) -> Option<(String, Way)> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<(String, Way)>>>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return known.clone();
    }
    let found = compute_method_way(method);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), found.clone());
    found
}

fn compute_method_way(method: &str) -> Option<(String, Way)> {
    // A 100 % market lot works its recipe's way (`trade_craft`).
    if let Some(recipe) = crate::trade_craft::recipe_of(method) {
        return compute_method_way(recipe);
    }
    let (family, tail) = method.split_once([':', '/'])?;
    let found = match family {
        "gather" => {
            let (_, item) = tail.split_once('/')?;
            if tail.starts_with("sheep/") {
                return ways(item).into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("shear")).map(|way| (item.to_string(), way));
            }
            if tail.starts_with("wheat/") {
                return ways(item).into_iter().find(|way| way.kind == WayKind::Harvest).map(|way| (item.to_string(), way));
            }
            (item.to_string(), gather_way_of(item)?)
        }
        "farm" => (tail.to_string(), gather_way_of(tail)?),
        "cook" if method.starts_with("cook/") => {
            let product = cooked_of(tail)?;
            let way = make_way_of(&product, &["cook"])?;
            (product, way)
        }
        "cook" => (tail.to_string(), make_way_of(tail, &["cook"])?),
        "bake" => {
            let mut way = make_way_of(tail, &["cook", "cook_prep"])?;
            let stages: Vec<String> = way.inputs.iter().map(|(name, _)| name.clone()).collect();
            let (carried, consumed): (Vec<(String, i64)>, Vec<(String, i64)>) = flatten_prep(&way.inputs).into_iter().partition(|(name, _)| returned_by(&stages, name));
            way.inputs = consumed;
            for (name, _) in carried {
                if !way.tools.contains(&name) {
                    way.tools.push(name);
                }
            }
            (tail.to_string(), way)
        }
        "smith" => (tail.to_string(), make_way_of(tail, &["smith"])?),
        "smelt" => (tail.to_string(), make_way_of(tail, &["smelt"])?),
        "spin" => (tail.to_string(), make_way_of(tail, &["spin"])?),
        "jewelry" => (tail.to_string(), make_way_of(tail, &["jewellery"])?),
        "runecraft" => (tail.to_string(), make_way_of(tail, &["runecraft"])?),
        "pottery" => (tail.to_string(), make_way_of(tail, &["pottery_fire", "pottery_shape"])?),
        "craft" => (tail.to_string(), make_way_of(tail, &["leather", "cook", "cook_prep"])?),
        "cut" => (tail.to_string(), make_way_of(tail, &["gem"])?),
        "mill" => (tail.to_string(), make_way_of(tail, &["mill"])?),
        "string" => make_way_using(tail, "string")?,
        "fire" => make_way_using(tail, "firemaking")?,
        "bury" => {
            let recipe = lifescape_gamedata::extra_recipes::by_kind("bury").into_iter().find(|row| row.inputs.iter().any(|(name, _)| name == tail))?;
            let mut way = Way {
                kind: WayKind::Make,
                detail: format!("bury {}", recipe.source),
                skill: Some(("prayer".to_string(), recipe.level)),
                inputs: recipe.inputs.clone(),
                tools: Vec::new(),
                coins: 0,
                rate: None,
                qty: None,
                combat: None,
                quest: None,
                quest_points: 0,
                sites: Vec::new(),
                file: String::new(),
                success: 1.0,
            };
            way.detail = format!("bury {tail}");
            (tail.to_string(), way)
        }
        "milk" => (tail.to_string(), ways(tail).into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("milk"))?),
        "dye" => (tail.to_string(), ways(tail).into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("aggie"))?),
        "superheat" => (tail.to_string(), superheat_way(tail)?),
        // `fill:<vessel>`: the filled vessel whose fill way takes it (the
        // way the dump knows: vessel in, water point sites). Live admin
        // 2026-11-15: no way known, the jug never staged, "missing jug".
        "fill" => ["bucket_water", "jug_water", "bowl_water", "vial_water"].iter().find_map(|filled| {
            let way = ways(filled).into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("fill") && way.inputs.first().is_some_and(|(input, _)| input == tail))?;
            Some((filled.to_string(), way))
        })?,
        // `enchant:<input>`: the spell's product (`config_recipes` spell
        // rows), the way the dump knows for it (runes + the jewel, magic
        // level). Live 2026-11-15: no way known, so no pricing and no
        // staging for the four F2P amulets.
        "enchant" => lifescape_gamedata::script_sources::config_recipes().iter().filter(|row| row.kind == "spell" && row.inputs.first().is_some_and(|input| input == tail)).find_map(|row| {
            let product = row.output.clone();
            let way = ways(&product).into_iter().find(|way| way.kind == WayKind::Spell && way.inputs.first().is_some_and(|(input, _)| input == tail))?;
            Some((product, way))
        })?,
        "telegrab" => (tail.to_string(), telegrab_way(tail)?),
        // D18: teleports, curses and bones to bananas, one cast per action.
        "teleport" | "curse" | "bananas" => return crate::spell_methods::spell_way(method),
        "ground" => {
            let mut merged = ways(tail).into_iter().filter(ground_workable).reduce(|mut merged, next| {
                merged.sites.extend(next.sites);
                merged
            })?;
            merged.sites.retain(spawn_workable);
            (tail.to_string(), merged)
        }
        "tan" => {
            let leather = ways("leather").into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("tanner"))?;
            ("leather".to_string(), leather)
        }
        _ => return None,
    };
    Some(found)
}

/// XP one action of `method` pays (`None`: no XP data). Gathers read
/// their tier tables, recipes their dump rows, fights their style model
/// (`sizing`), spells their dump row per CAST (plan-rendements §7 unit S:
/// a teleport or a curse yields nothing, bones to bananas pays per cast
/// whatever the bones it converts).
pub fn action_xp(method: &str) -> Option<f64> {
    let (product, way) = method_way(method)?;
    let (skill, _) = way.skill.as_ref()?;
    if way.kind == WayKind::Spell || way.detail.starts_with("telekinetic_grab") {
        let spell = way.detail.split_whitespace().next().unwrap_or("");
        let xp = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell).map(|row| row.xp)?;
        return (xp > 0).then_some(xp as f64 / 10.0);
    }
    // Anvil work pays per bar hammered (the dump's smith rows carry 0).
    if way.detail.starts_with("smith ") {
        let tenths: i64 = way.inputs.iter().filter_map(|(bar, count)| lifescape_gamedata::recipes::smith_xp_per_bar(bar).map(|per| per * count)).sum();
        return (tenths > 0).then_some(tenths as f64 / 10.0);
    }
    let tenths = match skill.as_str() {
        "fishing" => lifescape_gamedata::fishing::all_fish_tiers().into_iter().find_map(|tier| {
            let at = tier.products.iter().position(|name| *name == product)?;
            tier.exp.get(at).copied()
        })?,
        "mining" => lifescape_gamedata::rocks::all_rock_tiers().into_iter().find(|tier| tier.product == product).map(|tier| tier.exp)?,
        "woodcutting" => lifescape_gamedata::trees::all_tiers().into_iter().find(|tier| tier.product == product).map(|tier| tier.exp)?,
        _ => {
            let row = way.detail.split_whitespace().nth(1).unwrap_or("");
            let kind = way.detail.split_whitespace().next().unwrap_or("");
            let dumped = lifescape_gamedata::recipes::all_recipes(Some(kind)).into_iter().find(|recipe| recipe.source == row).map(|recipe| recipe.xp);
            let extra = || lifescape_gamedata::extra_recipes::by_kind(kind).into_iter().find(|recipe| recipe.source == row).map(|recipe| recipe.xp);
            let bury = || {
                lifescape_gamedata::extra_recipes::by_kind("bury")
                    .into_iter()
                    .find(|recipe| recipe.inputs.iter().any(|(name, _)| *name == product))
                    .map(|recipe| recipe.xp)
            };
            dumped.or_else(extra).or_else(bury)?
        }
    };
    (tenths > 0).then_some(tenths as f64 / 10.0)
}

#[cfg(test)]
mod tests {
    use super::{action_xp, best_combat_spell, magic_spell, method_way, spell_runes};

    #[test]
    fn every_method_family_finds_its_way() {
        for (method, product, skill) in [
            ("gather:fishing/raw_trout", "raw_trout", "fishing"),
            ("farm:iron_ore", "iron_ore", "mining"),
            ("gather:woodcut/willow_logs", "willow_logs", "woodcutting"),
            ("cook/raw_shrimp", "shrimp", "cooking"),
            ("smelt:bronze_bar", "bronze_bar", "smithing"),
            ("smith:bronze_sword", "bronze_sword", "smithing"),
            ("jewelry:gold_ring", "gold_ring", "crafting"),
            ("runecraft:airrune", "airrune", "runecraft"),
            ("cut:sapphire", "sapphire", "crafting"),
            ("fire:oak_logs", "ashes", "firemaking"),
            ("string:unstrung_gold_amulet", "strung_gold_amulet", "crafting"),
            ("spin:ball_of_wool", "ball_of_wool", "crafting"),
            ("pottery:pot_empty", "pot_empty", "crafting"),
            ("craft:leather_gloves", "leather_gloves", "crafting"),
            ("bury:bones", "bones", "prayer"),
        ] {
            let (made, way) = method_way(method).unwrap_or_else(|| panic!("{method} has a way"));
            assert_eq!(made, product, "{method}");
            assert_eq!(way.skill.as_ref().map(|(name, _)| name.as_str()), Some(skill), "{method}");
        }
        assert!(method_way("fight:melee/cows").is_none());
        assert!(method_way("market:buy").is_none());
    }

    /// XP per action from the server tables (tenths in the dumps).
    #[test]
    fn action_xp_reads_the_tables() {
        assert_eq!(action_xp("gather:fishing/raw_shrimp"), Some(10.0));
        assert_eq!(action_xp("gather:fishing/raw_salmon"), Some(70.0));
        assert_eq!(action_xp("gather:mining/copper_ore"), Some(17.5));
        assert_eq!(action_xp("gather:woodcut/logs"), Some(25.0));
        assert_eq!(action_xp("jewelry:gold_ring"), Some(15.0));
        assert_eq!(action_xp("runecraft:airrune"), Some(5.0));
        assert_eq!(action_xp("fire:logs"), Some(40.0));
        assert_eq!(action_xp("bury:big_bones"), Some(15.0));
        assert!(action_xp("smelt:bronze_bar").is_some_and(|xp| xp > 6.0));
        assert!(action_xp("cook/raw_shrimp").is_some_and(|xp| xp >= 30.0));
        // Anvil XP is per bar (server `xpperbar`): a platebody is 5 bars.
        assert_eq!(action_xp("smith:bronze_dagger"), Some(12.5));
        assert_eq!(action_xp("smith:bronze_platebody"), Some(62.5));
        assert_eq!(action_xp("smith:steel_platebody"), Some(187.5));
        assert_eq!(action_xp("smith:iron_scimitar"), Some(50.0));
    }

    #[test]
    fn magic_carries_its_spell() {
        assert_eq!(magic_spell("fight:magic/fire_strike/cows").as_deref(), Some("fire_strike"));
        assert_eq!(magic_spell("fight:magic/cows").as_deref(), Some("wind_strike"));
        assert_eq!(best_combat_spell(1), "wind_strike");
        assert_eq!(best_combat_spell(13), "fire_strike");
        assert_eq!(best_combat_spell(35), "fire_bolt");
        assert!(spell_runes("fire_strike").contains(&("firerune".to_string(), 3)));
    }
}
