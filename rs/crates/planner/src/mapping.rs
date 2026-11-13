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
    // Gatherers get their target resource first (args contract
    // 2026-11-13, user rule "precise instructions"): the script works that
    // resource only, never the best tier its level opens.
    let (script, mut head): (String, Vec<String>) = if let Some(fish) = method.strip_prefix("gather:fishing/") {
        ("fishing".to_string(), gather_head(fish, node, "fishing"))
    } else if let Some(logs) = method.strip_prefix("gather:woodcut/") {
        ("woodcut".to_string(), gather_head(logs, node, "woodcutting"))
    } else if let Some(ore) = method.strip_prefix("gather:mining/") {
        ("mining".to_string(), gather_head(ore, node, "mining"))
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
    } else if let Some(product) = method.strip_prefix("pottery:") {
        // The product rides first (args contract 2026-11-13: pot, pie dish
        // or bowl; audit: pie-dish and bowl tranches made pots).
        ("pottery".to_string(), vec![product.to_string()])
    } else if let Some(product) = method.strip_prefix("jewelry:") {
        ("smelt_jewelry".to_string(), vec![product.to_string()])
    } else if let Some(rune) = method.strip_prefix("runecraft:") {
        ("runecraft".to_string(), vec![rune.to_string()])
    } else if let Some(kind) = method.strip_prefix("pick:") {
        ("picking".to_string(), vec![kind.to_string()])
    } else if let Some(_product) = method.strip_prefix("mill:") {
        // Grain + empty pot ride every load: no positional.
        ("mill_grain".to_string(), Vec::new())
    } else if method.starts_with("milk:") {
        // Bucket in, dairy cow, bank loop: no positional.
        ("milk".to_string(), Vec::new())
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
        ("ranged".to_string(), fight_head(spot, node))
    } else if let Some(tail) = method.strip_prefix("fight:magic/") {
        // `fight:magic/<spell>/<spot>`: the spot first, then the spell the
        // planner sized the runes for (legacy `fight:magic/<spot>`: wind
        // strike).
        let spot = tail.rsplit('/').next().unwrap_or(tail);
        let spell = crate::method_way::magic_spell(method).unwrap_or_else(|| "wind_strike".to_string());
        let mut head = fight_head(spot, node);
        head.insert(1, "--spell".to_string());
        head.insert(2, spell);
        ("magic".to_string(), head)
    } else if let Some(gem) = method.strip_prefix("cut:") {
        // The script eats the uncut stone (`gem` recipe input).
        let uncut = crate::method_way::method_way(method).and_then(|(_, way)| way.inputs.first().map(|(name, _)| name.clone())).unwrap_or_else(|| gem.to_string());
        ("cut_gems".to_string(), vec![uncut])
    } else if let Some(dye) = method.strip_prefix("dye:") {
        ("dye".to_string(), vec![dye.to_string()])
    } else if let Some(item) = method.strip_prefix("ground:") {
        ("ground_pickup".to_string(), vec![item.to_string()])
    } else if method.starts_with("telegrab:") {
        ("telegrab_wine".to_string(), Vec::new())
    } else if let Some(bones) = method.strip_prefix("bury:") {
        // Prayer from the bank: withdraw the bones, bury, repeat.
        ("bury_loot".to_string(), vec![bones.to_string()])
    } else if method.starts_with("gather:sheep/") {
        ("shear".to_string(), Vec::new())
    } else if let Some(resource) = method.strip_prefix("gather:wheat/") {
        ("picking".to_string(), vec![resource.to_string()])
    } else if let Some(spot) = method.strip_prefix("fight:melee/") {
        ("melee".to_string(), fight_head(spot, node))
    } else if let Some(resource) = method.strip_prefix("farm:") {
        farm_args(resource)?
    } else if method == "market:buy" || method == "market:sell" {
        let (item, count) = obtain_pair(node)?;
        let side = if method == "market:buy" { "buy" } else { "sell" };
        ("market".to_string(), vec![side.to_string(), item, count])
    } else if method == "shop:buy" {
        // The planner's shop (`shop_site::pick_shop`), named from the
        // keeper's site: the script never picks a shop itself.
        let (item, count) = obtain_pair(node)?;
        let mut head = vec!["shopbuy".to_string(), item.clone(), count];
        if let Some(shop) = node.site.and_then(|site| crate::shop_site::shop_at(&item, site)) {
            head.push("--shop".to_string());
            head.push(shop);
        }
        ("market".to_string(), head)
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
        // An unknown quest key names no script: typed no-script, never
        // a silent Cook's Assistant.
        (quest_script(quest)?.to_string(), Vec::new())
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
    // Every reach tranche stops at its target level (`--level`), station
    // work included (cook, smelt, jewellery, runecraft, bury...).
    let field = !matches!(script.as_str(), "market" | "walkto" | "equip") && !script.starts_with("quest_");
    if field && !head.iter().any(|arg| arg == "--level") {
        head.extend(level_flag(node, None));
    }
    // The brain picked the site (`pick_site_gated`): every gatherer,
    // station, fight and shop works there (args contract 2026-11-13). A
    // guild site names the uniform the door checks on the body. Market
    // orders (buy, sell, flip, cleanup) trade through the daemon: no site,
    // no toll; only `shopbuy` walks to its keeper.
    let sited = !matches!(script.as_str(), "walkto" | "equip")
        && !script.starts_with("quest_")
        && (script != "market" || head.first().is_some_and(|sub| sub == "shopbuy"));
    if let Some((x, z, level)) = node.site {
        if sited {
            head.push("--site".to_string());
            head.push(format!("{x},{z},{level}"));
            for gate in lifescape_gamedata::access::gates_at(x, z, level) {
                if let lifescape_gamedata::access::Gate::Guild { uniform: Some(uniform), .. } = gate {
                    head.push("--wear".to_string());
                    head.push(uniform.to_string());
                }
            }
        }
    }
    // Quantity target of an obtain's production leg (`--want <item> N`,
    // N = count minus what the bot holds, recorded at launch in
    // `progress_current`): the script stops at N.
    if let NodeKind::ObtainItems { item, .. } = &node.kind {
        if !matches!(script.as_str(), "market" | "walkto" | "equip") && !script.starts_with("quest_") {
            head.push("--want".to_string());
            head.push(item.clone());
            head.push(remaining(node).to_string());
        }
    }
    // The site lies across the Al Kharid toll fence and the brain
    // budgeted the fare (`place_on_site`): the run may pay the gate.
    if node.toll && sited {
        head.push("toll".to_string());
    }
    head.extend(tail);
    Some((script, head))
}

/// `--level <skill> <N>` for a reach tranche: the script stops at the
/// target instead of spending its whole budget (gatherers name their own
/// skill; fights take the node's: attack, strength, defence, ranged...).
/// Nothing for other nodes (obtain, combat training) or a level below 2.
fn level_flag(node: &Node, skill: Option<&str>) -> Vec<String> {
    match &node.kind {
        NodeKind::ReachLevel { skill: aimed, level } if *level >= 2 => {
            vec!["--level".to_string(), skill.unwrap_or(aimed).to_string(), level.to_string()]
        }
        _ => Vec::new(),
    }
}

/// Gatherer head: the resource first, then the tranche's level target.
fn gather_head(resource: &str, node: &Node, skill: &str) -> Vec<String> {
    let mut head = vec![resource.to_string()];
    head.extend(level_flag(node, Some(skill)));
    head
}

/// Units an obtain still needs: its count minus what the bot held at
/// launch (`progress_current`), at least 1.
fn remaining(node: &Node) -> i64 {
    match &node.kind {
        NodeKind::ObtainItems { count, .. } => (*count - node.progress_current.max(0)).max(1),
        _ => 1,
    }
}

/// Fight scripts: the spot first (positional), then the level target.
fn fight_head(spot: &str, node: &Node) -> Vec<String> {
    let mut head = vec![spot.to_string()];
    head.extend(level_flag(node, None));
    head
}

/// Item and units a purchase or sale leg moves: what is still missing,
/// never the whole node count (the bot may hold part of it).
fn obtain_pair(node: &Node) -> Option<(String, String)> {
    match &node.kind {
        NodeKind::ObtainItems { item, .. } => Some((item.clone(), remaining(node).to_string())),
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
pub(crate) fn is_leather_recipe(product: &str) -> bool {
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
/// chickens for feathers). `None` when no activity yields the item: the
/// brain abandons typed (no-script) instead of hunting chickens for an
/// egg or a pickaxe.
fn farm_args(resource: &str) -> Option<(String, Vec<String>)> {
    // The fish itself rides as the target (`fishing raw_trout`): the
    // script works that tier's water, never the best tier its level
    // opens (b8jxrdd2: a trout quota bait-fished pike at fishing 50).
    if is_fish(resource) {
        return Some(("fishing".to_string(), vec![resource.to_string()]));
    }
    if resource.ends_with("_logs") || resource == "logs" {
        return Some(("woodcut".to_string(), vec![resource.to_string()]));
    }
    if resource.ends_with("_ore") || resource == "coal" || resource == "clay" {
        return Some(("mining".to_string(), vec![resource.to_string()]));
    }
    // Rune essence is mined only through Aubury's teleport (`essence:`).
    if resource == "blankrune" {
        return Some(("essence_run".to_string(), Vec::new()));
    }
    spot_for_drop(resource).map(|spot| ("melee".to_string(), vec![spot]))
}

/// True when `item` is an F2P catch (a fishing tier's product): raw
/// beef and raw chicken are drops, never fish.
pub(crate) fn is_fish(item: &str) -> bool {
    lifescape_gamedata::fishing::all_fish_tiers().iter().any(|tier| !tier.product_members && tier.products.contains(&item))
}

/// Combat spot whose NPCs drop `item` (`gamedata::combat_spots::drop_spots`):
/// the weakest UNGATED one first; a spot behind the Al Kharid toll only
/// when no ungated spot drops it (the site pick then budgets the fare
/// and the launch carries `toll`). Never underground, behind a key or a
/// boat (`kit::hunt::run_spot_with` refuses a bare key underground).
/// Live 2026-11-13: the first dropper by name sent "produce bones" to
/// al_kharid_warriors across the fence and magic staffs to the moss
/// giants. None when no such F2P spot hunts a dropper.
pub(crate) fn spot_for_drop(item: &str) -> Option<String> {
    lifescape_gamedata::combat_spots::drop_spots(item).first().map(|spot| spot.key.to_string())
}

fn quest_script(quest: &str) -> Option<&'static str> {
    Some(match quest {
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
        _ => return None,
    })
}

#[cfg(test)]
#[path = "mapping_contract_tests.rs"]
mod contract_tests;

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
        let egg = Node::new("n9", NodeKind::ObtainItems { item: "egg".to_string(), count: 1 }, Vec::new(), 1);
        assert_eq!(map_method("farm:egg", &egg, None), None, "no activity yields eggs: no chicken fallback");
    }

    /// Live 2026-11-13 (leo_king, bbkjz2vk, frank_lane, ryan_park: 125
    /// Usage + 123 abandons + 120 stuck goals in an hour): "produce bones"
    /// hunted al_kharid_warriors (first dropper by NAME) across the toll
    /// fence the brain never pays, and magic staffs sent a bare key to the
    /// moss giants underground (`run_spot_with` refuses both by design).
    /// A drop farm hunts the weakest ungated spot; a drop only gated spots
    /// yield is no farm leg (market/shop answer instead).
    #[test]
    fn drop_farms_never_target_gated_spots() {
        let bones = Node::new("n9", NodeKind::ObtainItems { item: "bones".to_string(), count: 50 }, Vec::new(), 50);
        let (script, args) = map_method("farm:bones", &bones, None).expect("chickens drop bones");
        assert_eq!((script.as_str(), args.first().map(String::as_str)), ("melee", Some("chickens")), "got {args:?}");
        let staff = Node::new("n9", NodeKind::ObtainItems { item: "magic_staff".to_string(), count: 1 }, Vec::new(), 1);
        assert_eq!(map_method("farm:magic_staff", &staff, None), None, "only moss giants (underground) drop it");
        for spot in lifescape_gamedata::combat_spots::all_spots() {
            let gated = spot.access.is_some() || spot.entrance.is_some();
            for item in ["bones", "big_bones", "magic_staff", "bronze_axe", "feather", "cowhide", "raw_beef"] {
                let node = Node::new("n9", NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
                if let Some((_, args)) = map_method(&format!("farm:{item}"), &node, None) {
                    assert!(!(gated && args.first().map(String::as_str) == Some(spot.key)), "farm:{item} hunts gated {}", spot.key);
                }
            }
        }
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
            ("pottery:pot_empty", "pottery", "pot_empty"),
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
                // No resource positional: only the obtain's quantity target.
                assert_eq!(args, vec!["--want".to_string(), "feather".to_string(), "2000".to_string(), "--node".to_string(), "n9".to_string()], "{method} takes no resource, got {args:?}");
            } else {
                assert_eq!(args.first().map(String::as_str), Some(resource), "{method} carries {resource} first, got {args:?}");
            }
            assert!(args.contains(&"n9".to_string()), "{method} carries the node, got {args:?}");
        }
        // `cook:` names the product but the script eats the raw input.
        let (_, args) = map_method("cook:trout", &obtain(), None).expect("cook maps");
        assert_eq!(args.first().map(String::as_str), Some("raw_trout"), "product resolves to raw, got {args:?}");
    }

    /// Melee, ranged and magic climbs stop at the target level like the
    /// gatherers (a tranche never overshoots by a whole budget); a combat
    /// training or a drop farm carries no level.
    #[test]
    fn fight_climbs_carry_their_level_target() {
        let climb = Node::new("n4", NodeKind::ReachLevel { skill: "attack".to_string(), level: 55 }, Vec::new(), 55);
        let (script, args) = map_method("fight:melee/cows", &climb, Some((0, 18000))).expect("melee maps");
        assert_eq!(script, "melee");
        let at = args.iter().position(|arg| arg == "--level").expect("level flag");
        assert_eq!(&args[at + 1..at + 3], &["attack".to_string(), "55".to_string()]);
        assert_eq!(args[0], "cows", "the spot stays the first positional");
        let ranged = Node::new("n5", NodeKind::ReachLevel { skill: "ranged".to_string(), level: 20 }, Vec::new(), 20);
        let (_, args) = map_method("fight:ranged/cows", &ranged, None).expect("ranged maps");
        assert!(args.windows(3).any(|window| window == ["--level", "ranged", "20"]), "got {args:?}");
        let combat = Node::new("n6", NodeKind::ReachCombat { level: 15 }, Vec::new(), 15);
        let (_, args) = map_method("fight:melee/chickens", &combat, None).expect("training maps");
        assert!(!args.contains(&"--level".to_string()), "combat level is no skill, got {args:?}");
        let (_, args) = map_method("fight:melee/chickens", &obtain(), None).expect("farm maps");
        assert!(!args.contains(&"--level".to_string()));
    }

    /// b8jxrdd2 (live 2026-11-13): a trout quota launched a bare
    /// `fishing` that bait-fished pike at level 50. The fish rides as the
    /// target; raw beef is a cow drop, never a fishing run.
    #[test]
    fn fish_farms_name_their_fish_and_meat_hunts() {
        let trout = Node::new("n9", NodeKind::ObtainItems { item: "raw_trout".to_string(), count: 100 }, Vec::new(), 100);
        let (script, args) = map_method("farm:raw_trout", &trout, None).expect("trout maps");
        assert_eq!(script, "fishing");
        assert_eq!(args.first().map(String::as_str), Some("raw_trout"), "the fish rides first, got {args:?}");
        assert!(args.windows(3).any(|window| window == ["--want", "raw_trout", "100"]), "quantity target, got {args:?}");
        let beef = Node::new("n9", NodeKind::ObtainItems { item: "raw_beef".to_string(), count: 10 }, Vec::new(), 10);
        let (script, args) = map_method("farm:raw_beef", &beef, None).expect("beef maps");
        assert_eq!((script.as_str(), args.first().map(String::as_str)), ("melee", Some("cows")), "got {args:?}");
    }

    /// Args contract 2026-11-13: gatherers get their resource first, an
    /// obtain asks only what is still missing (`--want`), purchases move
    /// the missing units, an unknown quest names no script.
    #[test]
    fn launches_carry_the_exact_order() {
        let mut ore = Node::new("n9", NodeKind::ObtainItems { item: "copper_ore".to_string(), count: 100 }, Vec::new(), 100);
        ore.progress_current = 40;
        let (script, args) = map_method("farm:copper_ore", &ore, None).expect("mining maps");
        assert_eq!((script.as_str(), args[0].as_str()), ("mining", "copper_ore"));
        assert!(args.windows(3).any(|window| window == ["--want", "copper_ore", "60"]), "got {args:?}");
        let climb = Node::new("n4", NodeKind::ReachLevel { skill: "mining".to_string(), level: 20 }, Vec::new(), 20);
        let (_, args) = map_method("gather:mining/iron_ore", &climb, None).expect("maps");
        assert_eq!(args[0], "iron_ore", "the tranche's ore rides first: {args:?}");
        assert!(!args.contains(&"--want".to_string()), "a climb has no quantity");
        let mut shears = Node::new("n5", NodeKind::ObtainItems { item: "shears".to_string(), count: 3 }, Vec::new(), 3);
        shears.progress_current = 1;
        let (_, args) = map_method("shop:buy", &shears, None).expect("shop maps");
        assert_eq!(&args[..3], &["shopbuy".to_string(), "shears".to_string(), "2".to_string()]);
        assert!(map_method("quest:no_such_quest", &shears, None).is_none(), "unknown quest: no script");
    }

    /// A site across the toll fence with the fare budgeted launches with
    /// `toll` (the scripts pay the gate); never without the flag.
    #[test]
    fn a_tolled_node_launches_with_toll() {
        let mut climb = Node::new("n4", NodeKind::ReachLevel { skill: "attack".to_string(), level: 20 }, Vec::new(), 20);
        let (_, args) = map_method("fight:melee/cows", &climb, None).expect("melee maps");
        assert!(!args.contains(&"toll".to_string()), "no fare budgeted, no toll: {args:?}");
        climb.toll = true;
        let (_, args) = map_method("fight:melee/cows", &climb, None).expect("melee maps");
        assert_eq!(args[0], "cows", "the spot stays first");
        assert!(args.contains(&"toll".to_string()), "got {args:?}");
        let mut gold = Node::new("n5", NodeKind::ObtainItems { item: "gold_ore".to_string(), count: 100 }, Vec::new(), 100);
        gold.site = Some((3294, 3287, 0));
        gold.toll = true;
        let (script, args) = map_method("farm:gold_ore", &gold, None).expect("mining maps");
        assert_eq!(script, "mining");
        assert!(args.contains(&"toll".to_string()) && args.contains(&"3294,3287,0".to_string()), "got {args:?}");
        // Bones: chickens first; the toll-gated warriors only rank last.
        let bones: Vec<&str> = lifescape_gamedata::combat_spots::drop_spots("bones").iter().map(|spot| spot.key).collect();
        assert_eq!(bones.first(), Some(&"chickens"));
        assert_eq!(bones.last(), Some(&"al_kharid_warriors"), "toll spots legal but last: {bones:?}");
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
