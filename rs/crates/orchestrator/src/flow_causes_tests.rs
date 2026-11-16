//! Golden balances, one per F2P skill (plan rendements §8): every input
//! AND every output of a run gets its cause, the units (P, C) follow.

use super::{balance, cause, cause_of, coin_cause, legacy_provenance, made_products, object_units, RawFlow, RunCtx};
use std::collections::BTreeMap;

fn flow(name: &str, kind: &str, delta: i64, skills: &[&str]) -> RawFlow {
    RawFlow {
        name: name.to_string(),
        display: name.to_string(),
        kind: kind.to_string(),
        delta,
        skills: skills.iter().map(|skill| skill.to_string()).collect(),
    }
}

fn ctx(activity: &str, method: &str, skills: &[&str], kills: u64) -> RunCtx {
    RunCtx {
        activity: activity.to_string(),
        planner_method: method.to_string(),
        run_skills: skills.iter().map(|skill| skill.to_string()).collect(),
        kills,
        made: Vec::new(),
    }
}

/// Balance of a run, as the report builds it (two passes: products
/// first, then every flow with the made list).
fn run(mut context: RunCtx, flows: &[RawFlow]) -> Vec<(String, &'static str, i64)> {
    context.made = made_products(flows, &context);
    let lines: Vec<(String, &'static str, i64)> = flows.iter().map(|flow| (flow.display.clone(), cause_of(flow, &context), flow.delta)).collect();
    balance(&lines)
}

fn line(name: &str, cause_name: &'static str, qty: i64) -> (String, &'static str, i64) {
    (name.to_string(), cause_name, qty)
}

fn units(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
    pairs.iter().map(|(name, qty)| (name.to_string(), *qty)).collect()
}

/// Melee with food and bones buried in the same run: the bones are loot
/// in AND prayer input out (the net diff saw nothing), food is eaten.
#[test]
fn melee_eats_and_buries() {
    let flows = [
        flow("bones", "pickup", 10, &[]),
        flow("bones", "bury", -10, &["prayer"]),
        flow("shrimp", "eat", -4, &["attack"]),
        flow("goblin_mail", "pickup", 2, &[]),
    ];
    let got = run(ctx("melee", "fight:melee/goblins", &["attack", "hitpoints", "prayer"], 10), &flows);
    assert_eq!(got, vec![line("bones", cause::INPUT, -10), line("bones", cause::LOOT, 10), line("goblin_mail", cause::LOOT, 2), line("shrimp", cause::FOOD, -4)]);
    assert_eq!(object_units(&got), (units(&[]), units(&[("bones", 10)])));
}

/// Ranged: arrows fired out of the quiver (with or without XP in the
/// window) and picked back up are ammo, never lost nor ground_spawn.
#[test]
fn ranged_spends_and_recovers_arrows() {
    let flows = [
        flow("bronze_arrow", "unattributed", -20, &[]),
        flow("bronze_arrow", "skill", -30, &["ranged"]),
        flow("bronze_arrow", "pickup", 25, &[]),
        flow("bones", "pickup", 3, &[]),
        flow("shrimp", "eat", -1, &[]),
    ];
    let got = run(ctx("ranged", "fight:ranged/chickens", &["ranged", "hitpoints"], 3), &flows);
    assert_eq!(got, vec![line("bones", cause::LOOT, 3), line("bronze_arrow", cause::AMMO, -50), line("bronze_arrow", cause::AMMO, 25), line("shrimp", cause::FOOD, -1)]);
}

/// Magic combat: the runes each cast burns, in or out of the cast window.
#[test]
fn magic_combat_burns_runes() {
    let flows = [
        flow("airrune", "cast", -30, &["magic"]),
        flow("mindrune", "cast", -30, &["magic"]),
        flow("airrune", "unattributed", -2, &[]),
        flow("bones", "pickup", 5, &[]),
    ];
    let got = run(ctx("magic", "fight:magic/wind_strike/chickens", &["magic", "hitpoints"], 5), &flows);
    assert_eq!(got, vec![line("airrune", cause::RUNE, -32), line("bones", cause::LOOT, 5), line("mindrune", cause::RUNE, -30)]);
}

/// Firemaking: the logs burnt are its input (the ashes stay on the
/// ground), the XP landing late or not.
#[test]
fn firemaking_burns_logs() {
    let flows = [flow("logs", "skill", -27, &["firemaking"]), flow("logs", "unattributed", -1, &[])];
    let got = run(ctx("firemaking", "fire:logs", &["firemaking"], 0), &flows);
    assert_eq!(got, vec![line("logs", cause::INPUT, -28)]);
    assert_eq!(object_units(&got), (units(&[]), units(&[("logs", 28)])));
}

/// Cooking: the raw fish burnt without XP is still an input; the burnt
/// fish is a by-product, then discarded.
#[test]
fn cooking_burns_some() {
    let flows = [
        flow("raw_shrimp", "skill", -8, &["cooking"]),
        flow("shrimp", "skill", 8, &["cooking"]),
        flow("raw_shrimp", "unattributed", -2, &[]),
        flow("burntfish1", "unattributed", 2, &[]),
        flow("burntfish1", "dropped", -2, &[]),
    ];
    let got = run(ctx("cook", "cook/raw_shrimp", &["cooking"], 0), &flows);
    assert_eq!(
        got,
        vec![line("burntfish1", cause::BYPRODUCT, 2), line("burntfish1", cause::DISCARDED, -2), line("raw_shrimp", cause::INPUT, -10), line("shrimp", cause::PRODUCT, 8)]
    );
    assert_eq!(object_units(&got), (units(&[("shrimp", 8)]), units(&[("raw_shrimp", 10)])));
}

/// Fishing: the catch is the product, bait and feathers the method's
/// consumables.
#[test]
fn fishing_spends_bait_and_feathers() {
    let flows = [
        flow("raw_sardine", "skill", 10, &["fishing"]),
        flow("fishing_bait", "skill", -10, &["fishing"]),
        flow("raw_trout", "skill", 5, &["fishing"]),
        flow("feather", "unattributed", -5, &[]),
    ];
    let got = run(ctx("fishing", "gather:fishing/raw_sardine", &["fishing"], 0), &flows);
    assert_eq!(got, vec![line("feather", cause::BAIT, -5), line("fishing_bait", cause::BAIT, -10), line("raw_sardine", cause::PRODUCT, 10), line("raw_trout", cause::PRODUCT, 5)]);
}

/// Mining and woodcutting: the product with its XP.
#[test]
fn gatherers_make_products() {
    let ore = run(ctx("mining", "gather:mining/copper_ore", &["mining"], 0), &[flow("copper_ore", "skill", 20, &["mining"])]);
    assert_eq!(ore, vec![line("copper_ore", cause::PRODUCT, 20)]);
    let logs = run(ctx("woodcut", "gather:woodcut/logs", &["woodcutting"], 0), &[flow("logs", "skill", 25, &["woodcutting"])]);
    assert_eq!(logs, vec![line("logs", cause::PRODUCT, 25)]);
}

/// Smelt then smith in one run: the bars are product AND input (the net
/// diff saw only part of them); the hammer never moves.
#[test]
fn smelt_and_smith_in_one_run() {
    let flows = [
        flow("copper_ore", "skill", -14, &["smithing"]),
        flow("tin_ore", "skill", -14, &["smithing"]),
        flow("bronze_bar", "skill", 14, &["smithing"]),
        flow("bronze_bar", "skill", -10, &["smithing"]),
        flow("bronze_dagger", "skill", 10, &["smithing"]),
    ];
    let got = run(ctx("smith", "smith:bronze_dagger", &["smithing"], 0), &flows);
    assert_eq!(
        got,
        vec![
            line("bronze_bar", cause::INPUT, -10),
            line("bronze_bar", cause::PRODUCT, 14),
            line("bronze_dagger", cause::PRODUCT, 10),
            line("copper_ore", cause::INPUT, -14),
            line("tin_ore", cause::INPUT, -14),
        ]
    );
    assert_eq!(
        object_units(&got),
        (units(&[("bronze_bar", 14), ("bronze_dagger", 10)]), units(&[("bronze_bar", 10), ("copper_ore", 14), ("tin_ore", 14)]))
    );
}

/// Pottery, two stages: shaped (unfired pot, crafting XP) then fired
/// (pot, crafting XP again): two products, two inputs.
#[test]
fn pottery_counts_both_stages() {
    let flows = [
        flow("softclay", "skill", -5, &["crafting"]),
        flow("pot_unfired", "skill", 5, &["crafting"]),
        flow("pot_unfired", "skill", -5, &["crafting"]),
        flow("pot_empty", "skill", 5, &["crafting"]),
    ];
    let got = run(ctx("pottery", "pottery:pot_empty", &["crafting"], 0), &flows);
    assert_eq!(object_units(&got), (units(&[("pot_empty", 5), ("pot_unfired", 5)]), units(&[("pot_unfired", 5), ("softclay", 5)])));
}

/// Leather crafting: the leather is the input, the thread a supply the
/// work uses up (one per five), the needle never moves.
#[test]
fn leather_uses_thread() {
    let flows = [
        flow("leather", "skill", -5, &["crafting"]),
        flow("leather_gloves", "skill", 5, &["crafting"]),
        flow("thread", "unattributed", -1, &[]),
    ];
    let got = run(ctx("leatherwork", "craft:leather_gloves", &["crafting"], 0), &flows);
    assert_eq!(got, vec![line("leather", cause::INPUT, -5), line("leather_gloves", cause::PRODUCT, 5), line("thread", cause::SUPPLY, -1)]);
}

/// Runecraft: essence in, runes out (never the spell's rune cost).
#[test]
fn runecraft_turns_essence_into_runes() {
    let flows = [flow("blankrune", "skill", -20, &["runecraft"]), flow("airrune", "skill", 20, &["runecraft"])];
    let got = run(ctx("runecraft", "runecraft:airrune", &["runecraft"], 0), &flows);
    assert_eq!(got, vec![line("airrune", cause::PRODUCT, 20), line("blankrune", cause::INPUT, -20)]);
    assert_eq!(object_units(&got), (units(&[("airrune", 20)]), units(&[("blankrune", 20)])));
}

/// Alchemy: the object alched is the unit (C), the runes burn, the coins
/// are the product but never a unit.
#[test]
fn alchemy_eats_the_item_and_runes() {
    let flows = [flow("iron_scimitar", "cast", -10, &["magic"]), flow("naturerune", "cast", -10, &["magic"]), flow("firerune", "cast", -30, &["magic"])];
    let context = ctx("alchemy", "alch:iron_scimitar", &["magic"], 0);
    let mut got = run(context.clone(), &flows);
    got.push(("Coins".to_string(), coin_cause("alch", 1_120, &context), 1_120));
    let got = balance(&got);
    assert_eq!(
        got,
        vec![line("Coins", cause::PRODUCT, 1_120), line("firerune", cause::RUNE, -30), line("iron_scimitar", cause::INPUT, -10), line("naturerune", cause::RUNE, -10)]
    );
    assert_eq!(object_units(&got), (units(&[]), units(&[("iron_scimitar", 10)])));
}

/// Enchant: the strung amulet in, the enchanted one out, runes burnt.
#[test]
fn enchant_turns_jewel_into_product() {
    let flows = [
        flow("strung_sapphire_amulet", "cast", -5, &["magic"]),
        flow("amulet_of_magic", "cast", 5, &["magic"]),
        flow("waterrune", "cast", -5, &["magic"]),
        flow("cosmicrune", "cast", -5, &["magic"]),
    ];
    let got = run(ctx("enchant_teleport", "enchant:strung_sapphire_amulet", &["magic"], 0), &flows);
    assert_eq!(
        got,
        vec![
            line("amulet_of_magic", cause::PRODUCT, 5),
            line("cosmicrune", cause::RUNE, -5),
            line("strung_sapphire_amulet", cause::INPUT, -5),
            line("waterrune", cause::RUNE, -5),
        ]
    );
}

/// Shop, trade, fee and death flows keep their action; what nothing
/// explains stays unattributed (an alert), never a yield.
#[test]
fn actions_and_alerts() {
    let context = ctx("melee", "", &["attack"], 1);
    assert_eq!(cause_of(&flow("bronze_axe", "shop", 1, &[]), &context), cause::SHOP_BOUGHT);
    assert_eq!(cause_of(&flow("logs", "shop", -3, &[]), &context), cause::SHOP_SOLD);
    assert_eq!(cause_of(&flow("naturerune", "trade", 20, &[]), &context), cause::TRADED);
    assert_eq!(cause_of(&flow("bronze_axe", "death", -1, &[]), &context), cause::DEATH);
    assert_eq!(cause_of(&flow("bronze_axe", "unattributed", -1, &[]), &context), cause::UNATTRIBUTED);
    assert_eq!(cause_of(&flow("bucket_empty", "unattributed", 1, &[]), &context), cause::UNATTRIBUTED);
    assert_eq!(coin_cause("fee", -10, &context), cause::FEE);
    assert_eq!(coin_cause("loot", 30, &context), cause::LOOT);
    assert_eq!(coin_cause("loot", 30, &ctx("ground_pickup", "", &[], 0)), cause::GROUND_SPAWN);
    assert_eq!(coin_cause("unattributed", -9_860, &context), cause::UNATTRIBUTED);
}

/// The D45 provenance the current aggregation reads, per cause.
#[test]
fn legacy_provenance_keeps_the_aggregation() {
    assert_eq!(legacy_provenance(cause::PRODUCT, 5, false), "produced");
    assert_eq!(legacy_provenance(cause::AMMO, -5, false), "consumed");
    assert_eq!(legacy_provenance(cause::FOOD, -1, true), "consumed", "lunch on a flip stays consumed");
    assert_eq!(legacy_provenance(cause::SHOP_BOUGHT, 1, true), "traded", "merchant legs read traded (D45)");
    assert_eq!(legacy_provenance(cause::DISCARDED, -2, false), "lost");
    assert_eq!(legacy_provenance(cause::UNATTRIBUTED, 3, false), "unattributed");
    assert_eq!(legacy_provenance(cause::UNATTRIBUTED, -3, false), "lost");
    for name in cause::ALL {
        assert!(lifescape_sdk::provenance::Provenance::parse(legacy_provenance(name, 1, false)).is_some(), "{name}");
    }
}
