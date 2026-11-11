//! Method → (script, args) table in Rust (never in JSON): the brain's
//! order to the worker. Engagement bounds ride as `--budget-min/max`
//! (deps excluded: only the activity script counts them); the node id
//! rides as `--node` so the session report correlates.

use crate::model::{Node, NodeKind};

/// Map one locked method to its script and CLI args. `None` means the
/// method names no executable: the brain fails loud, never guesses.
pub fn map_method(method: &str, node: &Node, budget: Option<(u64, u64)>) -> Option<(String, Vec<String>)> {
    let mut tail = vec!["--node".to_string(), node.id.clone()];
    if let Some((min_ticks, max_ticks)) = budget {
        tail.push("--budget-min".to_string());
        tail.push(min_ticks.to_string());
        tail.push("--budget-max".to_string());
        tail.push(max_ticks.to_string());
    }
    let (script, mut head): (String, Vec<String>) = if method.starts_with("gather:fishing/") {
        let level = match &node.kind {
            NodeKind::ReachLevel { level, .. } => *level,
            _ => 0,
        };
        let mut head = vec!["--level".to_string(), "fishing".to_string()];
        if level >= 2 {
            head.push(level.to_string());
        }
        // No resource positional: the gatherer works the best tier at or
        // below its live level inside the brain-fixed palier (D21: the
        // palier, not the exact tier, is the brain's order).
        ("fishing".to_string(), head)
    } else if method.starts_with("gather:woodcut/") {
        let level = match &node.kind {
            NodeKind::ReachLevel { level, .. } => *level,
            _ => 0,
        };
        let mut head = Vec::new();
        if level >= 2 {
            head.push("--level".to_string());
            head.push("woodcutting".to_string());
            head.push(level.to_string());
        }
        ("woodcut".to_string(), head)
    } else if method.starts_with("gather:mining/") {
        let level = match &node.kind {
            NodeKind::ReachLevel { level, .. } => *level,
            _ => 0,
        };
        let mut head = Vec::new();
        if level >= 2 {
            head.push("--level".to_string());
            head.push("mining".to_string());
            head.push(level.to_string());
        }
        ("mining".to_string(), head)
    } else if let Some(raw) = method.strip_prefix("cook/") {
        ("cook".to_string(), vec![raw.to_string()])
    } else if let Some(product) = method.strip_prefix("cook:") {
        // Chain-side `cook:` names the product (trout); the script eats
        // the raw input, resolved from the dump, never guessed.
        ("cook".to_string(), vec![cook_raw(product)])
    } else if let Some(product) = method.strip_prefix("bake:") {
        // Plain bread bakes in its own script; composed meals run in
        // the P3.3 meals script (meal name rides first there too).
        if product == "bread" {
            ("bake_bread".to_string(), Vec::new())
        } else {
            ("cook_meals".to_string(), vec![product.to_string()])
        }
    } else if let Some(bar) = method.strip_prefix("smelt:") {
        ("smelt".to_string(), vec![bar.to_string()])
    } else if let Some(product) = method.strip_prefix("smith:") {
        ("smith".to_string(), vec![product.to_string()])
    } else if let Some(hide) = method.strip_prefix("tan:") {
        // Tanning is leather's precondition: the leatherwork script
        // owns the tanner grid (dette P2.4: grid unverified, fails
        // typed until P3.4 maps it).
        ("leatherwork".to_string(), vec![hide.to_string()])
    } else if let Some(log) = method.strip_prefix("fire:") {
        ("firemaking".to_string(), vec![log.to_string()])
    } else if let Some(product) = method.strip_prefix("spin:") {
        // The wheel script eats the INPUT (wool only, flax members):
        // resolve it from the dump, like `cook:` resolves raw.
        ("spin".to_string(), vec![spin_input(&product)])
    } else if let Some(_product) = method.strip_prefix("pottery:") {
        // Pot only (P3.4): clay and water ride the same load, no
        // positional — the suffix documents the chain product.
        ("pottery".to_string(), Vec::new())
    } else if let Some(product) = method.strip_prefix("jewelry:") {
        ("smelt_jewelry".to_string(), vec![product.to_string()])
    } else if let Some(rune) = method.strip_prefix("runecraft:") {
        ("runecraft".to_string(), vec![rune.to_string()])
    } else if let Some(kind) = method.strip_prefix("pick:") {
        ("picking".to_string(), vec![kind.to_string()])
    } else if let Some(_product) = method.strip_prefix("mill:") {
        // Grain + empty pot ride every load: no positional.
        ("mill_grain".to_string(), Vec::new())
    } else if let Some(container) = method.strip_prefix("fill:") {
        ("fill_water".to_string(), vec![container.to_string()])
    } else if let Some(_kind) = method.strip_prefix("essence:") {
        // Aubury runs carry essence through the mine: no positional.
        ("essence_run".to_string(), Vec::new())
    } else if let Some(input) = method.strip_prefix("string:") {
        ("stringing".to_string(), vec![input.to_string()])
    } else if let Some(item) = method.strip_prefix("alch:") {
        // The script takes the ITEM (`alchemy <item>` picks low/high
        // by level itself): `alch:gold_ring`, never `alch:high`.
        ("alchemy".to_string(), vec![item.to_string()])
    } else if let Some(ore) = method.strip_prefix("superheat:") {
        ("superheat".to_string(), vec![ore.to_string()])
    } else if let Some(amulet) = method.strip_prefix("enchant:") {
        // The script resolves spell, level, runes and product from a
        // strung amulet (`enchant_for`): `enchant:strung_sapphire_amulet`.
        ("enchant_teleport".to_string(), vec![amulet.to_string()])
    } else if let Some(spot) = method.strip_prefix("fight:ranged/") {
        ("ranged".to_string(), vec![spot.to_string()])
    } else if let Some(spot) = method.strip_prefix("fight:magic/") {
        ("magic".to_string(), vec![spot.to_string()])
    } else if method.starts_with("gather:sheep/") {
        ("shear".to_string(), Vec::new())
    } else if let Some(resource) = method.strip_prefix("gather:wheat/") {
        ("picking".to_string(), vec![resource.to_string()])
    } else if let Some(spot) = method.strip_prefix("fight:melee/") {
        ("melee".to_string(), vec![spot.to_string()])
    } else if let Some(resource) = method.strip_prefix("farm:") {
        farm_args(resource)
    } else if method == "market:buy" || method == "market:sell" {
        let (item, count) = obtain_pair(node)?;
        let side = if method == "market:buy" { "buy" } else { "sell" };
        ("market".to_string(), vec![side.to_string(), item, count])
    } else if method == "shop:buy" {
        let (item, count) = obtain_pair(node)?;
        ("market".to_string(), vec!["shopbuy".to_string(), item, count])
    } else if method == "market:flip" {
        ("market".to_string(), vec!["flip".to_string()])
    } else if method == "space:cleanup" {
        ("market".to_string(), vec!["cleanup".to_string()])
    } else if let Some(recipe) = method.strip_prefix("craft:") {
        // Leather sews in the leatherwork script; everything else
        // bakes on a range (P3.3 owns composed dishes past plain fish).
        // Native `jewelry:`/`pottery:` methods bypass this fallback.
        if is_leather_recipe(recipe) {
            ("leatherwork".to_string(), vec![recipe.to_string()])
        } else {
            ("cook".to_string(), vec![recipe.to_string()])
        }
    } else if let Some(quest) = method.strip_prefix("quest:") {
        (quest_script(quest).to_string(), Vec::new())
    } else if let Some(area) = method.strip_prefix("access:") {
        // Safe areas are walk targets, not names: `goto`/`walkto` need
        // coordinates (a bare name fails Usage). Unknown area = None.
        let spot = match area {
            "lumbridge" => Some((3222, 3219)),
            "cooks_guild" => Some((3143, 3442)),
            "crafting_guild" => Some((2936, 3289)),
            "mining_guild" => Some((3017, 3339)),
            "champions_guild" => Some((3191, 3360)),
            "karamja" => Some((2924, 3180)),
            "al_kharid" => Some((3268, 3164)),
            "varrock" => Some((3205, 3429)),
            "falador" => Some((2966, 3378)),
            "draynor" => Some((3105, 3244)),
            "port_sarim" => Some((3040, 3215)),
            "barbarian_village" => Some((3080, 3424)),
            "edgeville" => Some((3086, 3500)),
            _ => None,
        };
        match spot {
            Some((x, z)) => ("walkto".to_string(), vec![x.to_string(), z.to_string()]),
            None => return None,
        }
    } else if let Some(style) = method.strip_prefix("equip:") {
        // The brain's EquipBest order: the script wears the best carried
        // kit through kit::gear (melee when the goal left it blank).
        ("equip".to_string(), vec![if style.is_empty() { "melee".to_string() } else { style.to_string() }])
    } else {
        return None;
    };
    head.extend(tail);
    Some((script, head))
}

fn obtain_pair(node: &Node) -> Option<(String, String)> {
    match &node.kind {
        NodeKind::ObtainItems { item, count } => Some((item.clone(), count.to_string())),
        _ => None,
    }
}

/// Raw input the cook script eats for a cooked `product` (dump `cook`
/// rows, e.g. trout → raw_trout). Falls back to the product itself so
/// an unknown name fails loud in the script, never here.
fn cook_raw(product: &str) -> String {
    lifescape_gamedata::recipes::making(product, Some("cook"))
        .into_iter()
        .flat_map(|recipe| recipe.inputs.into_iter().map(|(name, _)| name))
        .next()
        .unwrap_or_else(|| product.to_string())
}

/// True when the dump sews `product` from leather (the `craft:` router).
fn is_leather_recipe(product: &str) -> bool {
    !lifescape_gamedata::recipes::making(product, Some("leather")).is_empty()
}

/// Wheel input spinning into `product` (dump `spin` rows, e.g. the
/// ball of wool spins from wool). Falls back to the product itself so
/// an unknown name fails loud in the script, never here.
fn spin_input(product: &str) -> String {
    lifescape_gamedata::extra_recipes::by_kind("spin")
        .into_iter()
        .flat_map(|recipe| {
            let makes = recipe.outputs.iter().any(|(name, _)| name.eq_ignore_ascii_case(product));
            let input = recipe.inputs.first().map(|(name, _)| name.clone());
            if makes { input } else { None }
        })
        .next()
        .unwrap_or_else(|| product.to_string())
}

/// Farm routing: fish and logs/ore go to their gatherer; drops go to
/// the combat spot whose NPCs drop the item (goblins for bronze axes,
/// chickens for feathers); everything else falls back to chickens until
/// more activities branch off the socle.
fn farm_args(resource: &str) -> (String, Vec<String>) {
    if resource.starts_with("raw_") {
        return ("fishing".to_string(), Vec::new());
    }
    if resource.ends_with("_logs") {
        return ("woodcut".to_string(), Vec::new());
    }
    if resource.ends_with("_ore") || resource == "coal" || resource == "clay" {
        return ("mining".to_string(), Vec::new());
    }
    if let Some(spot) = spot_for_drop(resource) {
        return ("melee".to_string(), vec![spot]);
    }
    ("melee".to_string(), vec!["chickens".to_string()])
}

/// Combat spot whose NPCs drop `item` (debug-name prefix match: the
/// `goblin_greenarmour` drop row meets the `goblins` spot hunting plain
/// `goblin`s). None when no F2P spot hunts the dropper.
fn spot_for_drop(item: &str) -> Option<String> {
    let obj = lifescape_gamedata::objs_by_name(item).into_iter().next()?;
    let drops = lifescape_gamedata::drops::drops_for_obj(obj.obj_id);
    let mut npcs: Vec<String> = drops
        .iter()
        .filter(|drop| drop.npc_id >= 0)
        .map(|drop| drop.npc.clone())
        .collect();
    npcs.sort();
    npcs.dedup();
    for npc in npcs {
        for spot in lifescape_gamedata::combat_spots::all_spots() {
            if spot.npc_names.iter().any(|hunted| npc.starts_with(hunted)) {
                return Some(spot.key.to_string());
            }
        }
    }
    None
}

fn quest_script(quest: &str) -> &'static str {
    match quest {
        "cook" => "quest_cooks_assistant",
        "sheep" => "quest_sheep_shearer",
        "hetty" => "quest_witchs_potion",
        "haunted" => "quest_ernest_the_chicken",
        "romeojuliet" => "quest_romeo_and_juliet",
        "doric" => "quest_dorics_quest",
        "gobdip" => "quest_goblin_diplomacy",
        "imp" => "quest_imp_catcher",
        "runemysteries" => "quest_rune_mysteries",
        "blackarmgang" => "quest_shield_of_arrav",
        "blackknight" => "quest_black_knights_fortress",
        "demon" => "quest_demon_slayer",
        "dragon" => "quest_dragon_slayer",
        "squire" => "quest_knights_sword",
        "hunt" => "quest_pirates_treasure",
        "priest" => "quest_restless_ghost",
        "vampire" => "quest_vampire_slayer",
        "prince" => "quest_prince_ali_rescue",
        _ => "quest_cooks_assistant",
    }
}

#[cfg(test)]
mod tests {
    use super::map_method;
    use crate::model::{Node, NodeKind};

    fn obtain() -> Node {
        Node::new("n9", NodeKind::ObtainItems { item: "feather".to_string(), count: 2000 }, Vec::new(), 2000)
    }

    #[test]
    fn farm_routes_by_drop_source() {
        let (script, args) = map_method("farm:bronze_axe", &obtain(), None).expect("farm maps");
        assert_eq!(script, "melee");
        assert!(args.contains(&"goblins".to_string()), "axes drop from goblins, got {args:?}");
        let (script, args) = map_method("farm:feather", &obtain(), None).expect("farm maps");
        assert_eq!(script, "melee");
        assert!(args.contains(&"chickens".to_string()), "feathers drop from chickens, got {args:?}");
    }

    #[test]
    fn methods_map_to_scripts_with_node_and_budget() {        let budget = Some((6000, 12000));
        let (script, args) = map_method("market:buy", &obtain(), budget).expect("market maps");
        assert_eq!(script, "market");
        assert!(args.contains(&"buy".to_string()) && args.contains(&"feather".to_string()));
        assert!(args.contains(&"n9".to_string()) && args.contains(&"12000".to_string()), "node + budget ride along");

        let (script, _) = map_method("gather:woodcut/willow_logs", &obtain(), None).expect("woodcut maps");
        assert_eq!(script, "woodcut");
        let (script, _) = map_method("market:flip", &obtain(), budget).expect("flip maps");
        assert_eq!(script, "market");
        let cleanup = Node::new("n5", NodeKind::SpaceCleanup, Vec::new(), 0);
        let (script, args) = map_method("space:cleanup", &cleanup, None).expect("cleanup maps");
        assert_eq!(script, "market");
        assert!(args.contains(&"cleanup".to_string()), "cleanup subcommand rides along, got {args:?}");
        let (script, args) = map_method("shop:buy", &obtain(), None).expect("shop maps");
        assert_eq!(script, "market");
        assert!(args.contains(&"shopbuy".to_string()), "shopbuy subcommand rides along");
        let (script, _) = map_method("quest:cook", &obtain(), None).expect("quest maps");
        assert_eq!(script, "quest_cooks_assistant");
        assert!(map_method("equip:melee", &obtain(), None).is_some(), "equip wires to the equip script");
        let (script, args) = map_method("equip:ranged", &obtain(), None).expect("equip:ranged maps");
        assert_eq!(script, "equip");
        assert_eq!(args.first().map(String::as_str), Some("ranged"), "style rides first, got {args:?}");
        assert!(map_method("nope:x", &obtain(), None).is_none(), "unknown methods fail loud");
        assert!(map_method("nope:x", &obtain(), None).is_none(), "unknown methods fail loud");
    }

    /// P3.6: every P3 family names its script; the positional resource
    /// rides first (every script's convention), node and budget after.
    /// P3.1-P3.5 scripts land in parallel: a name missing from the
    /// registry fails typed at launch (`script_known`), never here.
    #[test]
    fn p3_families_branch_to_their_scripts() {
        for (method, script, resource) in [
            ("smelt:iron_bar", "smelt", "iron_bar"),
            ("smith:steel_platebody", "smith", "steel_platebody"),
            ("cook:trout", "cook", "raw_trout"),
            ("cook/raw_shrimp", "cook", "raw_shrimp"),
            ("bake:bread", "bake_bread", ""),
            ("tan:cow_hide", "leatherwork", "cow_hide"),
            ("fire:willow_logs", "firemaking", "willow_logs"),
            ("spin:ball_of_wool", "spin", "wool"),
            ("pottery:pot", "pottery", ""),
            ("jewelry:gold_ring", "smelt_jewelry", "gold_ring"),
            ("runecraft:airrune", "runecraft", "airrune"),
            ("pick:grain", "picking", "grain"),
            ("mill:pot_flour", "mill_grain", ""),
            ("fill:jug_water", "fill_water", "jug_water"),
            ("essence:blankrune", "essence_run", ""),
            ("string:unstrung_gold_amulet", "stringing", "unstrung_gold_amulet"),
            ("alch:gold_ring", "alchemy", "gold_ring"),
            ("superheat:iron_bar", "superheat", "iron_bar"),
            ("enchant:strung_sapphire_amulet", "enchant_teleport", "strung_sapphire_amulet"),
            ("fight:ranged/cows", "ranged", "cows"),
            ("fight:magic/cows", "magic", "cows"),
            ("gather:sheep/wool", "shear", ""),
            ("gather:wheat/grain", "picking", "grain"),
            ("craft:leather_gloves", "leatherwork", "leather_gloves"),
            ("craft:bread", "cook", "bread"),
        ] {
            let (mapped, args) = map_method(method, &obtain(), None).unwrap_or_else(|| panic!("{method} maps"));
            assert_eq!(mapped, script, "{method} names {script}");
            if resource.is_empty() {
                assert_eq!(args, vec!["--node".to_string(), "n9".to_string()], "{method} takes no resource, got {args:?}");
            } else {
                assert_eq!(args.first().map(String::as_str), Some(resource), "{method} carries {resource} first, got {args:?}");
            }
            assert!(args.contains(&"n9".to_string()), "{method} carries the node, got {args:?}");
        }
        // `cook:` names the product but the script eats the raw input.
        let (_, args) = map_method("cook:trout", &obtain(), None).expect("cook maps");
        assert_eq!(args.first().map(String::as_str), Some("raw_trout"), "product resolves to raw, got {args:?}");
    }

    #[test]
    fn access_maps_to_walkto_coords() {
        let access = Node::new("n3", NodeKind::AccessArea { area: "lumbridge".to_string() }, Vec::new(), 1);
        let (script, args) = map_method("access:lumbridge", &access, None).expect("access maps");
        assert_eq!(script, "walkto", "access walks, got {script}");
        assert_eq!(&args[..2], &["3222".to_string(), "3219".to_string()], "coords first, got {args:?}");
        assert!(map_method("access:nowhere", &access, None).is_none(), "unknown area fails typed");
    }
}
