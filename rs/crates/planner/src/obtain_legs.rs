//! Obtain legs from the gamedata obtain index (PRD3 lot 4): every way a
//! bot can come by an object becomes a candidate method — market buy,
//! every production way the scripts can run (gather, drop farming,
//! smithing, smelting, cooking, spinning, jewellery, stringing, milling,
//! runecrafting, pottery, leather, shearing, picking, filling, tanning,
//! milking, enchanting), and the NPC shop only when D-A allows it.

use lifescape_gamedata::obtain::WayKind;

use crate::model::{Alternative, Node, NodeKind};

/// One production leg: the method, the skill level it needs and what the
/// way carries from the server data (tools, quest, sites): the brain plans
/// every dependency from it, the script only executes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionLeg {
    pub method: String,
    pub skill: Option<(String, i64)>,
    /// Objects the leg consumes per unit (recursive obtains when absent).
    pub inputs: Vec<(String, i64)>,
    /// Objects carried, never consumed (hammer, mould, talisman, rod).
    pub tools: Vec<String>,
    /// F2P quest the way needs (planner key, `runemysteries`).
    pub quest: Option<String>,
    /// F2P sites (x, z, level) where the way happens; empty = anywhere.
    pub sites: Vec<(i64, i64, i64)>,
}

/// Method string for one production way of `item`, when a script runs it.
fn method_for(item: &str, kind: WayKind, detail: &str, inputs: &[(String, i64)]) -> Option<String> {
    let recipe_kind = detail.split_whitespace().next().unwrap_or("");
    let method = match kind {
        WayKind::Gather | WayKind::Drop => format!("farm:{item}"),
        WayKind::Make => match recipe_kind {
            "smith" => format!("smith:{item}"),
            "smelt" => format!("smelt:{item}"),
            // Composite food (bread dough, uncooked pies, stew, pizza,
            // cake) bakes through the meals scripts, never `cook`.
            "cook" if inputs.iter().any(|(input, _)| crate::method_way::prepared(input)) => format!("bake:{item}"),
            "cook" => format!("cook:{item}"),
            "gem" => format!("cut:{item}"),
            "spin" => format!("spin:{item}"),
            "jewellery" => format!("jewelry:{item}"),
            "string" => {
                let unstrung = inputs.iter().map(|(name, _)| name.as_str()).find(|name| *name != "ball_of_wool")?;
                format!("string:{unstrung}")
            }
            "mill" => format!("mill:{item}"),
            "runecraft" => format!("runecraft:{item}"),
            "pottery_shape" | "pottery_fire" => format!("pottery:{item}"),
            "leather" => format!("craft:{item}"),
            _ => return None,
        },
        WayKind::Harvest => {
            if detail.starts_with("shear") {
                "gather:sheep/wool".to_string()
            } else if let Some(field) = detail.strip_prefix("pick ") {
                format!("pick:{field}")
            } else if detail.starts_with("fill") {
                let container = inputs.first().map(|(name, _)| name.clone())?;
                format!("fill:{container}")
            } else if detail.starts_with("tanner") {
                format!("tan:{}", inputs.first().map(|(name, _)| name.as_str()).unwrap_or("cow_hide"))
            } else if detail.starts_with("milk") {
                format!("milk:{item}")
            } else if detail.starts_with("aggie") {
                format!("dye:{item}")
            } else {
                return None;
            }
        }
        WayKind::Spell if detail.starts_with("enchant") => format!("enchant:{}", inputs.first()?.0),
        WayKind::Spell if detail.starts_with("superheat_item") => format!("superheat:{item}"),
        // Wine of Zamorak: the monks attack a hand pickup, the grab spell
        // takes it from across the altar (`telegrab_wine`).
        WayKind::Spell if detail.starts_with("telekinetic_grab") => format!("telegrab:{item}"),
        WayKind::Ground => format!("ground:{item}"),
        _ => return None,
    };
    Some(method)
}

/// Production legs of `item` that map to a script, deduplicated.
/// Memoised per object for the process (CPU 2026-11-14: the bid and
/// seller cost recursion asked the same objects thousands of times a
/// minute, ~2-4 ms per smelted bar). Pure function of the static
/// gamedata (its own indexes are process-lifetime `OnceLock`s too), so a
/// cached answer is the same answer.
pub fn production_legs(item: &str) -> Vec<ProductionLeg> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<ProductionLeg>>>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(item) {
        return known.clone();
    }
    let legs = compute_production_legs(item);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(item.to_string(), legs.clone());
    legs
}

fn compute_production_legs(item: &str) -> Vec<ProductionLeg> {
    let probe = Node::new("probe", NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
    let mut out: Vec<ProductionLeg> = Vec::new();
    for way in crate::method_way::production_ways(item) {
        let farmable_drop = way.kind == WayKind::Drop && way.rate.is_some_and(|rate| rate > 0.0);
        let pickup = way.kind == WayKind::Ground && crate::method_way::ground_workable(&way);
        if !way.is_production() && !farmable_drop && !pickup {
            continue;
        }
        // A harvest with no F2P site is no way (maple: no F2P tree, live
        // 2026-11-15 `farm:maple_logs` woodcut "produced nothing").
        if way.kind == WayKind::Gather && way.sites.is_empty() {
            continue;
        }
        let Some(method) = method_for(item, way.kind, &way.detail, &way.inputs) else { continue };
        if crate::mapping::map_method(&method, &probe, None).is_none() {
            continue;
        }
        // A meal bakes from its base components (`cook_meals` assembles
        // the uncooked stage in the pack): flatten the prep chain.
        let inputs = if farmable_drop || pickup { Vec::new() } else { crate::method_way::flatten_prep(&way.inputs) };
        let quest = way.quest.as_deref().map(|quest| quest.strip_prefix("quest_").unwrap_or(quest).to_string());
        let leg = ProductionLeg { method, skill: way.skill.clone(), inputs, tools: way.tools.clone(), quest, sites: way.sites.clone() };
        if let Some(known) = out.iter_mut().find(|known| known.method == leg.method) {
            match (&known.skill, &leg.skill) {
                // `farm:<ore>` runs the skilled way (the mining script),
                // never the guards' drop listed before it: the leg carries
                // its gate and tools (a pickaxe), so the scorers price it.
                (None, Some(_)) => *known = leg,
                // Keep the easiest skill gate among duplicate ways.
                (Some((_, known_level)), Some((_, level))) if level < known_level => known.skill = leg.skill,
                _ => {}
            }
            continue;
        }
        out.push(leg);
    }
    out
}

/// Legs of an obtain node that name no executable (`map_method` refuses
/// them): legacy legs an older planner froze in a life file (pre-PRD3
/// `farm:<item>` for any object with a drop source, e.g. imps' wool balls
/// or dwarves' pickaxes, which no combat spot hunts). The `market:bid`
/// marker is no leg: it rides beside a production. Empty for other kinds.
pub fn dead_legs(node: &Node) -> Vec<String> {
    if !matches!(node.kind, NodeKind::ObtainItems { .. }) {
        return Vec::new();
    }
    node.alternatives
        .iter()
        .map(|alt| alt.method.clone())
        .filter(|method| method != "market:bid" && crate::mapping::map_method(method, node, None).is_none())
        .collect()
}

/// True when the object is F2P tradeable (the market universe).
pub fn tradeable_f2p(item: &str) -> bool {
    lifescape_gamedata::objs_by_name(item).into_iter().next().is_some_and(|entry| entry.tradeable && entry.members == Some(false))
}

/// Every candidate leg of an obtain node, market first (PRD3): the
/// market (non-Iron, tradeable), every production leg, then the NPC
/// shop when D-A allows it. Resolution orders them at ready time.
pub fn obtain_alternatives(item: &str, ironman: bool) -> Vec<Alternative> {
    let mut out = Vec::new();
    if !ironman && tradeable_f2p(item) {
        out.push(Alternative { method: "market:buy".to_string(), est_ticks: 0, est_coins: 0 });
    }
    for leg in production_legs(item) {
        out.push(Alternative { method: leg.method, est_ticks: 0, est_coins: 0 });
    }
    let (_, shopable) = crate::expand::source_flags(item);
    if shopable {
        out.push(Alternative { method: "shop:buy".to_string(), est_ticks: 0, est_coins: 0 });
    }
    // An NPC that sells through its dialogue (Karim's kebabs), under the
    // same merchant rule (`gamedata::dialogue_shops::allowed`).
    if !lifescape_gamedata::dialogue_shops::allowed(item).is_empty() {
        out.push(Alternative { method: "dialogue:buy".to_string(), est_ticks: 0, est_coins: 0 });
    }
    out
}

/// Legs of a production quota (`GoalKind::Produce`): every SKILLED
/// production way, never the market nor a shop (the product must be
/// made). A ground pickup or an unskilled drop farm trains nothing (live
/// 2026-11-15: smith milestones "produce iron_platelegs" ran
/// `ground:iron_platelegs` on a spawn 17 bots shared, failed 3 times and
/// redrew 22 smiths away; runecrafters picked mind runes off the floor).
/// A drop farm stays only for a product no skill makes (a warrior's cow
/// hides: the fight is the training).
pub fn produce_alternatives(item: &str) -> Vec<Alternative> {
    let legs: Vec<ProductionLeg> = production_legs(item).into_iter().filter(|leg| !leg.method.starts_with("ground:")).collect();
    let skilled = legs.iter().any(|leg| leg.skill.is_some());
    legs.into_iter()
        .filter(|leg| !skilled || leg.skill.is_some())
        .map(|leg| Alternative { method: leg.method, est_ticks: 0, est_coins: 0 })
        .collect()
}

/// Legs of an obtain node: production only for the goal's quota item
/// (`produce`), else every way (`obtain_alternatives`).
pub fn legs_for(item: &str, ironman: bool, produce: Option<&str>) -> Vec<Alternative> {
    if produce == Some(item) {
        produce_alternatives(item)
    } else {
        obtain_alternatives(item, ironman)
    }
}

#[cfg(test)]
mod tests {

    /// World invariant (audit 2026-11-15): a `shop:buy` leg exists only
    /// where an F2P keeper stocks the object at a known price. Live:
    /// black_full_helm planned as a shop buy no F2P world serves (1 646
    /// doomed launches).
    #[test]
    fn every_shop_leg_has_an_f2p_keeper() {
        let mut wrong = Vec::new();
        for obj in lifescape_gamedata::all_objs() {
            let Some(name) = obj.name else { continue };
            if obj.members != Some(false) || name.starts_with("cert_") {
                continue;
            }
            let shop_leg = super::obtain_alternatives(name, false).iter().any(|alt| alt.method == "shop:buy");
            if shop_leg && lifescape_gamedata::obtain::merchant_price(name).is_none() {
                wrong.push(name.to_string());
            }
        }
        assert!(wrong.is_empty(), "shop legs without an F2P keeper: {wrong:?}");
        // Valaine stocks it behind the 32 quest-point door: a leg the
        // resolver takes with the bot's quest points (user 2026-11-16, Q7).
        assert!(super::obtain_alternatives("black_full_helm", false).iter().any(|alt| alt.method == "shop:buy"));
        assert!(!super::obtain_alternatives("rune_full_helm", false).iter().any(|alt| alt.method == "shop:buy"), "no keeper at all");
    }
    use super::{compute_production_legs, obtain_alternatives, produce_alternatives, production_legs};

    /// A production quota is made, never picked up (iron platelegs have a
    /// Varrock ground spawn, mind runes four).
    #[test]
    fn quotas_never_run_a_pickup_or_an_unskilled_farm() {
        for item in ["iron_platelegs", "iron_platebody", "mindrune", "earthrune", "airrune"] {
            let legs: Vec<String> = produce_alternatives(item).into_iter().map(|leg| leg.method).collect();
            assert!(!legs.is_empty(), "{item} keeps a skilled leg");
            assert!(legs.iter().all(|method| !method.starts_with("ground:")), "{item}: {legs:?}");
        }
        assert!(produce_alternatives("iron_platelegs").iter().any(|leg| leg.method.starts_with("smith")), "smithing makes them");
        assert!(produce_alternatives("mindrune").iter().any(|leg| leg.method.starts_with("runecraft")), "the altar makes them");
    }

    /// Spawns behind a quest-locked door are never a pickup leg: the
    /// Draynor Manor closet (rubber tube) and Wydin's back room (white
    /// apron) — live admin 2026-11-15, DoorDidNotOpen on both.
    #[test]
    fn quest_locked_rooms_offer_no_pickup() {
        for item in ["rubber_tube", "white_apron", "iron_platelegs", "iron_platebody", "iron_sword"] {
            assert!(!methods(item).contains(&format!("ground:{item}")), "{item}: {:?}", methods(item));
        }
    }

    fn methods(item: &str) -> Vec<String> {
        production_legs(item).into_iter().map(|leg| leg.method).collect()
    }

    /// The memo answers exactly what a fresh computation does, cold and
    /// warm (superheat, smelt, recipes, gathers, unknown objects).
    #[test]
    fn memoised_legs_equal_fresh_legs() {
        for item in ["iron_bar", "bronze_bar", "logs", "ball_of_wool", "bronze_dagger", "no_such_object"] {
            let fresh = compute_production_legs(item);
            assert_eq!(production_legs(item), fresh, "{item} cold");
            assert_eq!(production_legs(item), fresh, "{item} warm");
        }
    }

    #[test]
    fn every_kind_of_production_becomes_a_method() {
        assert!(methods("bronze_sword").contains(&"smith:bronze_sword".to_string()));
        assert!(methods("bronze_bar").contains(&"smelt:bronze_bar".to_string()));
        assert!(methods("ball_of_wool").contains(&"spin:ball_of_wool".to_string()));
        assert!(methods("wool").contains(&"gather:sheep/wool".to_string()));
        assert!(methods("logs").contains(&"farm:logs".to_string()));
        assert!(methods("feather").contains(&"farm:feather".to_string()), "chicken drops farm");
        assert!(methods("pot_flour").contains(&"mill:pot_flour".to_string()));
        assert!(methods("strung_gold_amulet").contains(&"string:unstrung_gold_amulet".to_string()));
        assert!(methods("airrune").contains(&"runecraft:airrune".to_string()));
        assert!(methods("amulet_of_strength").contains(&"enchant:strung_ruby_amulet".to_string()));
        let smith = production_legs("bronze_sword").into_iter().find(|leg| leg.method == "smith:bronze_sword").expect("smith leg");
        assert_eq!(smith.skill.as_ref().map(|(skill, _)| skill.as_str()), Some("smithing"));
        assert!(smith.inputs.iter().any(|(input, _)| input == "bronze_bar"));
    }

    /// Iron ore drops from guards (listed first) and is mined at 15:
    /// `farm:iron_ore` runs the mining script, so its leg carries the
    /// mining gate and the pickaxe (a miner prices its own ore).
    #[test]
    fn a_mined_ore_leg_carries_the_mining_gate_and_pickaxe() {
        for (ore, level) in [("iron_ore", 15), ("copper_ore", 1)] {
            let mined = leg(ore, &format!("farm:{ore}"));
            assert_eq!(mined.skill, Some(("mining".to_string(), level)), "{ore}");
            assert_eq!(mined.tools, vec!["bronze_pickaxe".to_string()], "{ore}");
        }
    }

    fn leg(item: &str, method: &str) -> super::ProductionLeg {
        production_legs(item).into_iter().find(|leg| leg.method == method).unwrap_or_else(|| panic!("{item}: no {method} in {:?}", methods(item)))
    }

    /// Audit 2026-11-13 (A12): gem cutting, composite food, dyes, ground
    /// spawns, the wine grab and superheat are production legs with their
    /// server inputs, tools and levels.
    #[test]
    fn every_f2p_family_is_a_leg() {
        let sapphire = leg("sapphire", "cut:sapphire");
        assert_eq!(sapphire.tools, vec!["chisel".to_string()]);
        assert_eq!(sapphire.inputs, vec![("uncut_sapphire".to_string(), 1)]);
        let bread = leg("bread", "bake:bread");
        assert!(bread.inputs.contains(&("pot_flour".to_string(), 1)) && bread.inputs.contains(&("jug_water".to_string(), 1)), "{:?}", bread.inputs);
        let pie = leg("redberry_pie", "bake:redberry_pie");
        for base in ["pot_flour", "jug_water", "piedish", "redberries"] {
            assert!(pie.inputs.iter().any(|(input, _)| input == base), "pie bakes from {base}: {:?}", pie.inputs);
        }
        assert_eq!(leg("yellowdye", "dye:yellowdye").inputs, vec![("onion".to_string(), 2)], "Aggie: two onions");
        let heat = leg("iron_bar", "superheat:iron_bar");
        assert_eq!(heat.skill, Some(("magic".to_string(), 43)));
        assert!(heat.inputs.iter().any(|(input, _)| input == "naturerune"), "{:?}", heat.inputs);
        assert!(heat.inputs.iter().any(|(input, _)| input == "iron_ore"));
        let wine = leg("wine_of_zamorak", "telegrab:wine_of_zamorak");
        assert_eq!(wine.skill, Some(("magic".to_string(), 33)));
        assert!(wine.inputs.iter().any(|(input, _)| input == "lawrune"));
        // Ground spawns a bot may sweep: free, ungated but for the Karamja
        // boat (a fare, user 2026-11-16), a bank in reach.
        let swept: Vec<&str> = ["hammer", "tinderbox", "bucket_empty", "pot_empty", "jug_empty", "chisel", "bronze_pickaxe", "shears", "spade", "cabbage", "egg", "grain"]
            .into_iter()
            .filter(|item| methods(item).contains(&format!("ground:{item}")))
            .collect();
        assert!(!swept.is_empty(), "some tool or food spawn is sweepable");
        for item in swept {
            let (_, way) = crate::method_way::method_way(&format!("ground:{item}")).expect("ground way");
            for (x, z, level) in way.sites {
                assert!(lifescape_gamedata::access::gates_at(x, z, level).iter().all(lifescape_gamedata::access::fare_only), "{item} spawn ({x},{z},{level}) is gated");
            }
        }
    }

    #[test]
    fn a_harvest_without_an_f2p_site_is_no_leg() {
        assert!(production_legs("maple_logs").is_empty(), "no F2P maple tree");
        assert!(production_legs("yew_logs").iter().any(|leg| leg.method == "farm:yew_logs"));
    }

    #[test]
    fn market_leads_and_shop_follows_the_keepers() {
        let sword: Vec<String> = obtain_alternatives("bronze_sword", false).into_iter().map(|alt| alt.method).collect();
        assert_eq!(sword.first().map(String::as_str), Some("market:buy"), "market first");
        assert!(sword.contains(&"shop:buy".to_string()), "a keeper stocks it (user 2026-11-15)");
        let shears: Vec<String> = obtain_alternatives("shears", false).into_iter().map(|alt| alt.method).collect();
        assert!(shears.contains(&"shop:buy".to_string()));
        let iron: Vec<String> = obtain_alternatives("bronze_sword", true).into_iter().map(|alt| alt.method).collect();
        assert!(!iron.contains(&"market:buy".to_string()), "Iron never trades");
    }
}
