use super::{alch_bid_ceiling, alch_spell, alch_value, best_alch, caster, FIRE_RUNE, NATURE_RUNE};

fn runes(item: &str) -> f64 {
    match item {
        "naturerune" => 100.0,
        "firerune" => 4.0,
        _ => 1.0,
    }
}

/// The server spell table names the runes: one nature and five fire
/// for high alchemy, three fire for low.
#[test]
fn spells_read_the_server_table() {
    assert_eq!(alch_spell(20), None);
    let low = alch_spell(21).expect("low alchemy");
    let high = alch_spell(55).expect("high alchemy");
    assert!(!low.high && high.high);
    let count = |spell: &super::AlchSpell, rune: &str| spell.runes.iter().find(|(name, _)| name == rune).map(|(_, count)| *count);
    assert_eq!(count(&high, NATURE_RUNE), Some(1), "{high:?}");
    assert_eq!(count(&high, FIRE_RUNE), Some(5), "{high:?}");
    assert_eq!(count(&low, FIRE_RUNE), Some(3), "{low:?}");
    assert!((high.xp - 65.0).abs() < 1e-9, "{high:?}");
}

/// A steel platebody alchs for 1200 (cost 2000 × 0.6): a caster nets
/// 1200 − 100 − 20, so alching beats selling it at 900 and loses to a
/// buyer at 1200; a staff of fire saves the fire runes.
#[test]
fn alching_beats_selling_under_the_net() {
    assert_eq!(alch_value("steel_platebody", true), Some(1200));
    let mage = caster(60, false, &runes).expect("caster");
    assert!((mage.net("steel_platebody").unwrap() - 1080.0).abs() < 1e-9);
    assert!(mage.profit("steel_platebody", 900.0).unwrap() > 0.0);
    assert!(mage.profit("steel_platebody", 1200.0).unwrap() < 0.0);
    let staff = caster(60, true, &runes).expect("caster");
    assert!((staff.net("steel_platebody").unwrap() - 1100.0).abs() < 1e-9, "no fire runes with the staff");
    assert_eq!(caster(10, true, &runes), None, "magic 10 casts no alchemy");
}

/// The best stock to alch: the highest profit per cast, enough units,
/// never a loss.
#[test]
fn the_best_alch_is_the_most_profitable_stock() {
    let mage = caster(60, false, &runes).expect("caster");
    let stock = vec![
        ("steel_platebody".to_string(), 30, 1000.0),
        ("iron_platebody".to_string(), 40, 50.0),
        ("bronze_dagger".to_string(), 500, 1.0),
        ("rune_scimitar".to_string(), 2, 1.0),
    ];
    let pick = best_alch(&mage, &stock).expect("a profitable alch");
    assert_eq!(pick.item, "iron_platebody", "336 − 120 − 50 beats 1080 − 1000: {pick:?}");
    assert_eq!(pick.casts, 40);
    assert!(pick.profit_per_6000 > 100_000.0, "{pick:?}");
    assert!((pick.xp_per_6000 - 78_000.0).abs() < 1.0, "1200 casts × 65 XP: {pick:?}");
    assert!(best_alch(&mage, &[("bronze_dagger".to_string(), 500, 1.0)]).is_none(), "a dagger alchs at a loss");
}

/// The alch floor: a caster bids at most the net less its time.
#[test]
fn a_caster_bids_under_the_net() {
    let mage = caster(60, false, &runes).expect("caster");
    assert_eq!(alch_bid_ceiling(&mage, "steel_platebody", 2.0), Some(1070));
    assert_eq!(alch_bid_ceiling(&mage, "bronze_dagger", 2.0), None);
}
