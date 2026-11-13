//! Obtain legs from the gamedata obtain index (PRD3 lot 4): every way a
//! bot can come by an object becomes a candidate method — market buy,
//! every production way the scripts can run (gather, drop farming,
//! smithing, smelting, cooking, spinning, jewellery, stringing, milling,
//! runecrafting, pottery, leather, shearing, picking, filling, tanning,
//! milking, enchanting), and the NPC shop only when D-A allows it.

use lifescape_gamedata::obtain::{ways, WayKind};

use crate::model::{Alternative, Node, NodeKind};

/// One production leg: the method and the skill level it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionLeg {
    pub method: String,
    pub skill: Option<(String, i64)>,
    /// Objects the leg consumes per unit (recursive obtains when absent).
    pub inputs: Vec<(String, i64)>,
}

/// Method string for one production way of `item`, when a script runs it.
fn method_for(item: &str, kind: WayKind, detail: &str, inputs: &[(String, i64)]) -> Option<String> {
    let recipe_kind = detail.split_whitespace().next().unwrap_or("");
    let method = match kind {
        WayKind::Gather | WayKind::Drop => format!("farm:{item}"),
        WayKind::Make => match recipe_kind {
            "smith" => format!("smith:{item}"),
            "smelt" => format!("smelt:{item}"),
            "cook" => format!("cook:{item}"),
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
            } else {
                return None;
            }
        }
        WayKind::Spell if detail.starts_with("enchant") => format!("enchant:{}", inputs.first()?.0),
        _ => return None,
    };
    Some(method)
}

/// Production legs of `item` that map to a script, deduplicated.
pub fn production_legs(item: &str) -> Vec<ProductionLeg> {
    let probe = Node::new("probe", NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
    let mut out: Vec<ProductionLeg> = Vec::new();
    for way in ways(item) {
        let farmable_drop = way.kind == WayKind::Drop && way.rate.is_some_and(|rate| rate > 0.0);
        if !way.is_production() && !farmable_drop {
            continue;
        }
        let Some(method) = method_for(item, way.kind, &way.detail, &way.inputs) else { continue };
        if crate::mapping::map_method(&method, &probe, None).is_none() {
            continue;
        }
        if let Some(known) = out.iter_mut().find(|leg| leg.method == method) {
            // Keep the easiest skill gate among duplicate ways.
            if let (Some((_, known_level)), Some((_, level))) = (&known.skill, &way.skill) {
                if level < known_level {
                    known.skill = way.skill.clone();
                }
            }
            continue;
        }
        let inputs = if farmable_drop { Vec::new() } else { way.inputs.clone() };
        out.push(ProductionLeg { method, skill: way.skill.clone(), inputs });
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
    out
}

/// Legs of a production quota (`GoalKind::Produce`): every production
/// way, never the market nor a shop (the product must be made).
pub fn produce_alternatives(item: &str) -> Vec<Alternative> {
    production_legs(item).into_iter().map(|leg| Alternative { method: leg.method, est_ticks: 0, est_coins: 0 }).collect()
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
    use super::{obtain_alternatives, production_legs};

    fn methods(item: &str) -> Vec<String> {
        production_legs(item).into_iter().map(|leg| leg.method).collect()
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

    #[test]
    fn market_leads_and_shop_follows_d_a() {
        let sword: Vec<String> = obtain_alternatives("bronze_sword", false).into_iter().map(|alt| alt.method).collect();
        assert_eq!(sword.first().map(String::as_str), Some("market:buy"), "market first");
        assert!(!sword.contains(&"shop:buy".to_string()), "producible: no merchant");
        let shears: Vec<String> = obtain_alternatives("shears", false).into_iter().map(|alt| alt.method).collect();
        assert!(shears.contains(&"shop:buy".to_string()));
        let iron: Vec<String> = obtain_alternatives("bronze_sword", true).into_iter().map(|alt| alt.method).collect();
        assert!(!iron.contains(&"market:buy".to_string()), "Iron never trades");
    }
}
