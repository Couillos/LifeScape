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
