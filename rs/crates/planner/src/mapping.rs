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
    } else if let Some(resource) = method.strip_prefix("cook/") {
        ("cook".to_string(), vec![resource.to_string()])
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
    } else if method == "market:flip" || method == "space:cleanup" {
        ("market".to_string(), vec!["flip".to_string()])
    } else if let Some(recipe) = method.strip_prefix("craft:") {
        ("cook".to_string(), vec![recipe.to_string()])
    } else if let Some(quest) = method.strip_prefix("quest:") {
        (quest_script(quest).to_string(), Vec::new())
    } else if let Some(area) = method.strip_prefix("access:") {
        ("goto".to_string(), vec![area.to_string()])
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
    fn methods_map_to_scripts_with_node_and_budget() {
        let budget = Some((6000, 12000));
        let (script, args) = map_method("market:buy", &obtain(), budget).expect("market maps");
        assert_eq!(script, "market");
        assert!(args.contains(&"buy".to_string()) && args.contains(&"feather".to_string()));
        assert!(args.contains(&"n9".to_string()) && args.contains(&"12000".to_string()), "node + budget ride along");

        let (script, _) = map_method("gather:woodcut/willow_logs", &obtain(), None).expect("woodcut maps");
        assert_eq!(script, "woodcut");
        let (script, _) = map_method("market:flip", &obtain(), budget).expect("flip maps");
        assert_eq!(script, "market");
        let (script, args) = map_method("shop:buy", &obtain(), None).expect("shop maps");
        assert_eq!(script, "market");
        assert!(args.contains(&"shopbuy".to_string()), "shopbuy subcommand rides along");
        let (script, _) = map_method("quest:cook", &obtain(), None).expect("quest maps");
        assert_eq!(script, "quest_cooks_assistant");
        assert!(map_method("equip:melee", &obtain(), None).is_none(), "equip is not a script order");
        assert!(map_method("nope:x", &obtain(), None).is_none(), "unknown methods fail loud");
    }
}
