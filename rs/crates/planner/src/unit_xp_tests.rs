use super::{carrier_kinds, derived_xp, unit_xp};

fn units(rows: &[(&str, &str, f64)]) -> Vec<(String, String, f64)> {
    rows.iter().map(|(kind, subject, count)| (kind.to_string(), subject.to_string(), *count)).collect()
}

fn xp_of(rows: &[(String, f64)], skill: &str) -> f64 {
    rows.iter().find(|(name, _)| name == skill).map(|(_, xp)| *xp).unwrap_or(0.0)
}

/// Derived XP equals the dump for a sample of families: the unit XP is
/// `action_xp` (dump tenths / 10) and only the family's carrier pays.
#[test]
fn derived_xp_is_the_dump() {
    let fish = derived_xp("gather:fishing/raw_shrimp", "", &units(&[("product", "raw_shrimp", 10.0), ("product", "raw_anchovies", 2.0)]), 20);
    assert_eq!(xp_of(&fish, "fishing"), 10.0 * 10.0 + 2.0 * 40.0);
    let fire = derived_xp("fire:logs", "", &units(&[("input", "logs", 5.0), ("product", "ashes", 5.0)]), 1);
    assert_eq!(fire, vec![("firemaking".to_string(), 200.0)], "the burnt log pays, the ashes never");
    let bury = derived_xp("bury:big_bones", "", &units(&[("input", "big_bones", 4.0)]), 1);
    assert_eq!(xp_of(&bury, "prayer"), 60.0);
    let smith = derived_xp("smith:bronze_platebody", "", &units(&[("product", "bronze_platebody", 2.0), ("input", "bronze_bar", 10.0)]), 18);
    assert_eq!(smith, vec![("smithing".to_string(), 125.0)]);
    let rc = derived_xp("runecraft:airrune", "", &units(&[("input", "blankrune", 27.0)]), 1);
    assert_eq!(xp_of(&rc, "runecraft"), 27.0 * 5.0, "per essence, whatever the runes per essence");
}

/// Spells without a product: teleports and curses pay per cast; alchemy
/// per object; superheat both Magic and Smithing.
#[test]
fn spells_without_a_product_pay_per_cast() {
    let teleport = derived_xp("teleport:varrock_teleport", "", &units(&[("cast", "varrock_teleport", 3.0)]), 25);
    assert_eq!(xp_of(&teleport, "magic"), 3.0 * 35.0);
    let curse = derived_xp("curse:confuse/chickens", "", &units(&[("cast", "confuse", 2.0)]), 3);
    assert_eq!(xp_of(&curse, "magic"), 2.0 * 13.0);
    let alch = derived_xp("alch:iron_dagger", "", &units(&[("input", "iron_dagger", 10.0), ("product", "coins", 350.0)]), 21);
    assert_eq!(xp_of(&alch, "magic"), 10.0 * 31.0, "low alchemy 31 per object");
    let superheat = derived_xp("superheat:iron_bar", "", &units(&[("product", "iron_bar", 1.0)]), 43);
    assert_eq!(xp_of(&superheat, "magic"), 53.0);
    assert!(xp_of(&superheat, "smithing") > 0.0);
}

/// Combat: damage × 4 to the style's skill, × 1.33 to hitpoints; magic
/// 2 per damage plus the spell's base per cast.
#[test]
fn combat_pays_by_damage_and_style() {
    let melee = derived_xp("fight:melee/cows", "melee:strength", &units(&[("damage", "cow", 100.0), ("kill", "cow", 12.0)]), 10);
    assert_eq!(xp_of(&melee, "strength"), 400.0);
    assert!((xp_of(&melee, "hitpoints") - 133.0).abs() < 1e-9);
    assert_eq!(xp_of(&melee, "attack"), 0.0);
    let buried = derived_xp("fight:melee/cows", "melee:strength", &units(&[("damage", "cow", 10.0), ("input", "bones", 3.0)]), 10);
    assert!((xp_of(&buried, "prayer") - 3.0 * 4.5).abs() < 1e-9, "bones buried during a fight pay Prayer");
    let controlled = derived_xp("fight:melee/men", "melee:controlled", &units(&[("damage", "man", 30.0)]), 10);
    for skill in ["attack", "strength", "defence", "hitpoints"] {
        assert!((xp_of(&controlled, skill) - 39.9).abs() < 1e-9, "{skill}: {controlled:?}");
    }
    let ranged = derived_xp("fight:ranged/chickens", "ranged", &units(&[("damage", "chicken", 30.0)]), 10);
    assert_eq!(xp_of(&ranged, "ranged"), 120.0);
    let magic = derived_xp("fight:magic/wind_strike/chickens", "magic", &units(&[("damage", "chicken", 10.0), ("cast", "wind_strike", 6.0)]), 1);
    assert_eq!(xp_of(&magic, "magic"), 10.0 * 2.0 + 6.0 * 5.5);
    assert_eq!(carrier_kinds("fight:magic/wind_strike/chickens", "magic"), &["damage", "cast"]);
    assert!(unit_xp("gather:woodcut/logs", "", "input", "logs", 1).is_empty(), "no carrier, no XP");
}
