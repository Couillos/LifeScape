//! Provenance de production : forge, fonte, cuisine, runes, poterie,
//! filage, meunerie, feu, alchimie ; combat et marché jamais touchés.

use super::production_flows;

fn deltas(rows: &[(&str, i64)]) -> Vec<(String, i64)> {
    rows.iter().map(|(name, qty)| (name.to_string(), *qty)).collect()
}

fn flows(activity: &str, rows: &[(&str, i64)], skills: &[&str]) -> super::ProductionFlows {
    let gained: Vec<String> = skills.iter().map(|skill| skill.to_string()).collect();
    production_flows(activity, &deltas(rows), &|skill: &str| gained.iter().any(|known| known == skill))
}

/// Live bst29bh9 `smith` : 4 barres de fer mangées, 2 casques forgés
/// avec l'XP de forge (rapporté « lost » et « ground_spawn »).
#[test]
fn smith_makes_helms_from_bars() {
    let run = flows("smith", &[("iron_bar", -4), ("iron_full_helm", 2)], &["smithing"]);
    assert!(run.produced.contains("iron_full_helm"), "{run:?}");
    assert!(run.consumed.contains("iron_bar"), "{run:?}");
    // Sans XP de forge ni barre mangée : rien n'est fabriqué.
    let idle = flows("smith", &[("iron_full_helm", 2)], &[]);
    assert!(idle.produced.is_empty(), "{idle:?}");
}

/// Fonte : minerais mangés (le fer rate une fois sur deux), barres faites.
#[test]
fn smelt_makes_bars_from_ores() {
    let run = flows("smelt", &[("copper_ore", -14), ("tin_ore", -14), ("bronze_bar", 14)], &["smithing"]);
    assert!(run.produced.contains("bronze_bar"), "{run:?}");
    assert!(run.consumed.contains("copper_ore") && run.consumed.contains("tin_ore"), "{run:?}");
    let iron = flows("smelt", &[("iron_ore", -10), ("iron_bar", 5)], &["smithing"]);
    assert!(iron.produced.contains("iron_bar") && iron.consumed.contains("iron_ore"), "{iron:?}");
}

/// Cuisine : le cru est mangé, le cuit ET le brûlé sont des produits.
#[test]
fn cook_makes_food_and_burnt_byproducts() {
    let run = flows("cook", &[("raw_shrimp", -10), ("shrimp", 8), ("burntfish1", 2)], &["cooking"]);
    assert!(run.produced.contains("shrimp"), "{run:?}");
    assert!(run.produced.contains("burntfish1"), "burnt fish is the recipe's byproduct: {run:?}");
    assert!(run.consumed.contains("raw_shrimp"), "{run:?}");
}

/// Runes : l'essence mangée, les runes faites, le talisman (outil) jamais.
#[test]
fn runecraft_makes_runes_from_essence() {
    let run = flows("runecraft", &[("blankrune", -27), ("airrune", 27)], &["runecraft"]);
    assert!(run.produced.contains("airrune"), "{run:?}");
    assert!(run.consumed.contains("blankrune"), "{run:?}");
    assert!(!run.consumed.contains("air_talisman"));
}

/// Poterie : argile et eau mangées à travers les étapes du même run
/// (argile molle, pot cru), pots faits.
#[test]
fn pottery_makes_pots_through_intermediates() {
    let run = flows("pottery", &[("softclay", -6), ("pot_empty", 6)], &["crafting"]);
    assert!(run.produced.contains("pot_empty"), "{run:?}");
    assert!(run.consumed.contains("softclay"), "{run:?}");
    let raw = flows("pottery", &[("clay", -6), ("jug_water", -6), ("pot_empty", 6), ("jug_empty", 6)], &["crafting"]);
    assert!(raw.produced.contains("pot_empty"), "{raw:?}");
    assert!(raw.consumed.contains("clay") && raw.consumed.contains("jug_water"), "{raw:?}");
}

/// Filage : la laine mangée, la pelote faite.
#[test]
fn spin_makes_balls_from_wool() {
    let run = flows("spin", &[("wool", -28), ("ball_of_wool", 28)], &["crafting"]);
    assert!(run.produced.contains("ball_of_wool"), "{run:?}");
    assert!(run.consumed.contains("wool"), "{run:?}");
}

/// Meunerie sans XP : le script du run est celui de la jambe du
/// planificateur ; grain et pot vide mangés, farine faite.
#[test]
fn mill_makes_flour_without_xp() {
    let run = flows("mill_grain", &[("grain", -14), ("pot_empty", -14), ("pot_flour", 14)], &[]);
    assert!(run.produced.contains("pot_flour"), "{run:?}");
    assert!(run.consumed.contains("grain") && run.consumed.contains("pot_empty"), "{run:?}");
    // Le même gain sous un autre script sans XP n'est pas fabriqué.
    let other = flows("woodcut", &[("grain", -14), ("pot_empty", -14), ("pot_flour", 14)], &[]);
    assert!(other.produced.is_empty(), "{other:?}");
}

/// Feu : les bûches brûlées sont mangées, aucun produit dans le sac.
#[test]
fn firemaking_consumes_logs() {
    let run = flows("firemaking", &[("logs", -27)], &["firemaking"]);
    assert!(run.consumed.contains("logs"), "{run:?}");
    assert!(run.produced.is_empty());
}

/// Alchimie : pièces produites, objet et runes mangés.
#[test]
fn alchemy_produces_coins() {
    let run = flows("alchemy", &[("coins", 1200), ("iron_platebody", -10), ("naturerune", -10), ("firerune", -50)], &["magic"]);
    assert!(run.coins_produced, "{run:?}");
    assert!(run.consumed.contains("iron_platebody") && run.consumed.contains("naturerune") && run.consumed.contains("firerune"), "{run:?}");
}

/// Combat et marché ne fabriquent rien ici.
#[test]
fn melee_and_market_never_produce() {
    assert!(flows("melee", &[("bones", 3), ("cowhide", 2)], &["attack"]).produced.is_empty());
    assert!(flows("market", &[("iron_bar", -4), ("iron_full_helm", 2)], &["smithing"]).produced.is_empty());
}
