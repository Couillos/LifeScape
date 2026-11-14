//! Provenance de production d'un rapport de tâche, lue dans les données
//! du jeu (même vérité que le planificateur) et jamais dans une liste à la
//! main : une activité de transformation (forge, fonte, cuisine, poterie,
//! filage, artisanat, runes, meunerie, traite, tonte, cueillette, sorts…)
//! étiquette ses produits `produced` et les intrants qu'elle a mangés
//! `consumed`, au lieu de `ground_spawn` / `lost` (live bst29bh9 `smith` :
//! « Iron bar lost -4, Iron full helm ground_spawn +2 »).
//!
//! Règle, pour un objet gagné pendant le run :
//! - une voie de production (`method_way::production_ways`, `is_production`)
//!   le fabrique, et
//! - cette voie a travaillé PENDANT ce run : l'XP de sa compétence est
//!   montée, ou une jambe du planificateur (`obtain_legs::production_legs`)
//!   qui la travaille se lance par le script même du run (`map_method`,
//!   meunerie, traite, tonte : sans XP), et
//! - au moins un de ses intrants a baissé (ou elle n'en a aucun : récolte).
//!
//! Ses intrants baissés sont alors `consumed`. Un intrant baissé sans
//! produit gagné (bûches brûlées, runes d'un sort, objet alché) est
//! `consumed` quand une recette qui le mange a travaillé (XP de sa
//! compétence) ou quand un sort qui le brûle a payé son XP de magie.
//! Le poisson brûlé est le sous-produit de sa recette de cuisine.
//! L'alchimie (XP de magie, runes d'un sort d'alchimie mangées) produit
//! ses pièces. Combat et marché ne passent jamais ici (butin, échanges).
//! Pur : noms de debug en entrée, ensembles de noms en sortie.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata::obtain::{Way, WayKind};
use lifescape_planner::model::{Node, NodeKind};

/// Produits et intrants d'un run (noms de debug).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProductionFlows {
    pub produced: HashSet<String>,
    pub consumed: HashSet<String>,
    /// Pièces gagnées par l'alchimie (sort payé, runes mangées).
    pub coins_produced: bool,
}

/// Activités qui ne fabriquent rien : butin de combat, jambes de marché.
fn production_activity(activity: &str) -> bool {
    !activity.is_empty() && activity != "melee" && !crate::rates_service::is_trading_method(activity)
}

/// Script lancé par une méthode du planificateur (sonde d'obtention).
fn method_script(method: &str, product: &str) -> Option<String> {
    let probe = Node::new("probe", NodeKind::ObtainItems { item: product.to_string(), count: 1 }, Vec::new(), 1);
    lifescape_planner::map_method(method, &probe, None).map(|(script, _)| script)
}

/// Une jambe de production de `product` se lance par le script `activity`
/// (jamais le ramassage au sol, jamais une chasse de drop).
fn leg_runs_here(product: &str, activity: &str) -> bool {
    lifescape_planner::obtain_legs::production_legs(product).iter().any(|leg| {
        let drop_hunt = leg.method.starts_with("farm:") && leg.skill.is_none();
        !leg.method.starts_with("ground:") && !drop_hunt && method_script(&leg.method, product).as_deref() == Some(activity)
    })
}

/// Intrants de base que la voie a mangés pendant le run : ses intrants
/// baissés, et à travers un intermédiaire fabriqué puis mangé dans le
/// même run (argile → argile molle → pot cru → pot ; farine → pâte →
/// fond de tarte), les intrants baissés de sa propre voie. `None` quand
/// la voie n'a rien mangé ; `Some([])` pour une récolte sans intrant.
fn fed_inputs(way: &Way, decreased: &HashSet<&str>, depth: u8) -> Option<Vec<String>> {
    if way.inputs.is_empty() {
        return Some(Vec::new());
    }
    let mut eaten: Vec<String> = Vec::new();
    for (input, _) in &way.inputs {
        if decreased.contains(input.as_str()) {
            eaten.push(input.clone());
        } else if depth > 0 {
            let stage = lifescape_gamedata::obtain::ways(input)
                .into_iter()
                .filter(|stage| stage.is_production() && stage.kind != WayKind::Note && !stage.inputs.is_empty())
                .find_map(|stage| fed_inputs(&stage, decreased, depth - 1));
            eaten.extend(stage.unwrap_or_default());
        }
    }
    eaten.sort();
    eaten.dedup();
    (!eaten.is_empty()).then_some(eaten)
}

/// La voie a travaillé pendant ce run (son XP est montée ou son script
/// est celui du run) et a mangé des intrants (aucun : récolte) : les
/// intrants de base mangés.
fn way_worked(way: &Way, product: &str, activity: &str, decreased: &HashSet<&str>, xp_gained: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    if !way.is_production() {
        return None;
    }
    // Un appât acheté dans le run peut finir en hausse nette : une pêche
    // payée en XP reste une récolte nourrie.
    let eaten = match fed_inputs(way, decreased, PREP_DEPTH) {
        Some(eaten) => eaten,
        None if way.kind == WayKind::Gather => Vec::new(),
        None => return None,
    };
    let worked = way.skill.as_ref().is_some_and(|(skill, _)| xp_gained(skill)) || leg_runs_here(product, activity);
    worked.then_some(eaten)
}

/// Profondeur des étapes intermédiaires d'un même run (la chaîne de
/// préparation d'une tarte compte quatre étapes).
const PREP_DEPTH: u8 = 4;

/// Recette de cuisine dont `burnt` est le raté, quand un de ses intrants
/// a baissé : ses intrants baissés.
fn burnt_inputs(burnt: &str, decreased: &HashSet<&str>) -> Option<Vec<String>> {
    lifescape_gamedata::recipes::all_recipes(Some("cook"))
        .into_iter()
        .filter(|recipe| recipe.burnt.as_deref() == Some(burnt))
        .find(|recipe| recipe.inputs.iter().any(|(input, _)| decreased.contains(input.as_str())))
        .map(|recipe| recipe.inputs.into_iter().map(|(input, _)| input).filter(|input| decreased.contains(input.as_str())).collect())
}

/// Produits des recettes qui mangent `input` (recettes dumpées et
/// recettes complémentaires : runes, feu, filage, poterie…).
fn products_using(input: &str) -> Vec<String> {
    let mut products: Vec<String> = lifescape_gamedata::recipes::using(input, None)
        .into_iter()
        .flat_map(|recipe| recipe.outputs.into_iter().map(|(name, _)| name))
        .collect();
    for row in lifescape_gamedata::extra_recipes::all_extra() {
        if row.inputs.iter().any(|(name, _)| name == input) {
            products.extend(row.outputs.into_iter().map(|(name, _)| name));
        }
    }
    products.sort();
    products.dedup();
    products
}

/// Classe les flux de production d'un run : `deltas` = (nom de debug,
/// quantité nette), `xp_gained(compétence)` = l'XP de cette compétence
/// (nom serveur : `smithing`, `runecraft`…) est montée pendant le run.
pub fn production_flows(activity: &str, deltas: &[(String, i64)], xp_gained: &dyn Fn(&str) -> bool) -> ProductionFlows {
    let mut flows = ProductionFlows::default();
    if !production_activity(activity) {
        return flows;
    }
    let decreased: HashSet<&str> = deltas.iter().filter(|(_, delta)| *delta < 0).map(|(name, _)| name.as_str()).collect();
    for (product, delta) in deltas {
        if *delta <= 0 || product == "coins" {
            continue;
        }
        let mut made = false;
        for way in lifescape_planner::method_way::production_ways(product) {
            let Some(eaten) = way_worked(&way, product, activity, &decreased, xp_gained) else { continue };
            made = true;
            flows.consumed.extend(eaten);
            // Un outil usé par le travail (fil de l'artisanat du cuir).
            flows.consumed.extend(way.tools.iter().filter(|tool| decreased.contains(tool.as_str())).cloned());
        }
        if !made {
            if let Some(inputs) = burnt_inputs(product, &decreased) {
                made = true;
                flows.consumed.extend(inputs);
            }
        }
        if made {
            flows.produced.insert(product.clone());
        }
    }
    let spells = lifescape_gamedata::spells::all_spells();
    let magic = xp_gained("magic");
    for (input, delta) in deltas {
        if *delta >= 0 || flows.consumed.contains(input) || input == "coins" {
            continue;
        }
        // Runes brûlées par un sort qui a payé son XP de magie.
        if magic && spells.iter().any(|spell| spell.runes.iter().any(|(rune, _)| rune == input)) {
            flows.consumed.insert(input.clone());
            continue;
        }
        let worked = products_using(input).iter().any(|product| {
            lifescape_planner::method_way::production_ways(product).iter().any(|way| {
                way.inputs.iter().any(|(name, _)| name == input) && way_worked(way, product, activity, &decreased, xp_gained).is_some()
            })
        });
        if worked {
            flows.consumed.insert(input.clone());
        }
    }
    // Alchimie : pièces gagnées avec l'XP de magie et les runes d'un sort
    // d'alchimie mangées ; l'objet changé en pièces est consommé.
    let coins_gained = deltas.iter().any(|(name, delta)| name == "coins" && *delta > 0);
    let alched = magic
        && spells
            .iter()
            .filter(|spell| spell.spell.contains("alchemy"))
            .any(|spell| spell.runes.iter().any(|(rune, _)| decreased.contains(rune.as_str())));
    if coins_gained && alched {
        flows.coins_produced = true;
        for (input, delta) in deltas {
            if *delta < 0 && input != "coins" {
                flows.consumed.insert(input.clone());
            }
        }
    }
    flows
}

#[cfg(test)]
#[path = "report_flows_tests.rs"]
mod tests;
