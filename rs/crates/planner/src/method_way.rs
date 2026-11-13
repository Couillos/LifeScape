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

/// Best F2P combat spell castable at `level` (strike, bolt, blast).
pub fn best_combat_spell(level: u8) -> String {
    lifescape_gamedata::spells::all_spells()
        .into_iter()
        .filter(|spell| !spell.members && spell.maxhit.is_some() && spell.level >= 1 && spell.level <= i64::from(level) && spell.name.is_some())
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
pub fn method_way(method: &str) -> Option<(String, Way)> {
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
        "bake" => (tail.to_string(), make_way_of(tail, &["cook", "cook_prep"])?),
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
                sites: Vec::new(),
                file: String::new(),
                success: 1.0,
            };
            way.detail = format!("bury {tail}");
            (tail.to_string(), way)
        }
        "milk" => (tail.to_string(), ways(tail).into_iter().find(|way| way.kind == WayKind::Harvest && way.detail.starts_with("milk"))?),
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
/// (`sizing`).
pub fn action_xp(method: &str) -> Option<f64> {
    let (product, way) = method_way(method)?;
    let (skill, _) = way.skill.as_ref()?;
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
