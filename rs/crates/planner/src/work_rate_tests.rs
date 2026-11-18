use super::{prior_rate, WorkRate};

fn rate() -> WorkRate {
    WorkRate {
        actions_6000: 1000.0,
        xp_6000: vec![("smithing".to_string(), 12_500.0)],
        flows_6000: vec![
            ("iron_ore".to_string(), "input".to_string(), -1000.0),
            ("iron_bar".to_string(), "product".to_string(), 500.0),
            ("coins".to_string(), "fee".to_string(), -10.0),
        ],
    }
}

/// Ticks per XP, XP per attempt and the inputs per XP all follow from
/// one rate: half the iron smelts fail, so an attempt pays 6.25 XP and
/// a bar's 12.5 XP costs two ores; a fee is no consumption.
#[test]
fn one_rate_gives_time_xp_and_consumption() {
    let read = rate();
    assert!((read.ticks_per_xp("smithing").expect("smithing") - 0.48).abs() < 1e-9);
    assert!((read.xp_per_action("smithing").expect("per attempt") - 12.5).abs() < 1e-9);
    assert_eq!(read.burned_per_xp("smithing"), vec![("iron_ore".to_string(), 0.08)]);
    assert!(read.ticks_per_xp("mining").is_none(), "pays no mining XP");
    assert!(read.burned_per_xp("mining").is_empty());
}

/// The prior's XP is DERIVED from its units (`unit_xp`): an anvil press
/// pays the dump's XP once per bronze dagger, its time from the server
/// action ticks and the bank walk, never the old 5 + 60/27 constants.
#[test]
fn the_prior_rate_derives_its_xp_from_units() {
    let read = prior_rate("smith:bronze_dagger", 1).expect("bronze dagger prior");
    let per_action = read.xp_per_action("smithing").expect("smithing XP");
    assert!((per_action - 12.5).abs() < 1e-6, "one bar, 12.5 XP: {per_action}");
    let ticks = read.ticks_per_xp("smithing").expect("ticks") * per_action;
    assert!(ticks > 5.0 && ticks < 20.0, "a press plus its share of the bank walk: {ticks}");
    assert!(prior_rate("no_such:method", 1).is_none(), "no prior, no rate");
}

/// oscar_cole / ray_79 2026-11-17: a fight that buries the bones it
/// loots (+100 loot, -100 input per window) read "missing bones" and its
/// climb bid for 1000 bones; arrows fired then picked back read only
/// their net loss. What the method itself takes in nets out of its burn.
#[test]
fn what_the_method_takes_in_nets_out_of_its_burn() {
    let fight = WorkRate {
        actions_6000: 100.0,
        xp_6000: vec![("ranged".to_string(), 4000.0)],
        flows_6000: vec![
            ("bones".to_string(), "loot".to_string(), 100.0),
            ("bones".to_string(), "input".to_string(), -100.0),
            ("bronze_arrow".to_string(), "ammo".to_string(), -50.0),
            ("bronze_arrow".to_string(), "ammo".to_string(), 20.0),
        ],
    };
    let burn = fight.burned_per_xp("ranged");
    assert!(burn.iter().all(|(item, _)| item != "bones"), "bones it buries come from its own loot: {burn:?}");
    let arrows = burn.iter().find(|(item, _)| item == "bronze_arrow").map(|(_, per_xp)| *per_xp).expect("arrows burnt");
    assert!((arrows - 30.0 / 4000.0).abs() < 1e-9, "net loss of arrows: {arrows}");
}
