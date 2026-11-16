use super::{all_spell_methods, cast_spell, cast_xp, curse_method, debuff_spells, spell_fits_spot, spell_spots, teleport_spells, BANANA_BONES_PER_CAST};
use crate::method_way::{action_xp, best_combat_spell, best_combat_spell_at, method_way};
use crate::sizing::{tranche_actions, tranche_consumption, tranche_need, xp_per_action};

/// The sets come from the spell dump: three F2P teleports, four NPC
/// debuffs (members rows never).
#[test]
fn the_dump_names_the_spell_sets() {
    assert_eq!(teleport_spells(), vec!["varrock_teleport", "lumbridge_teleport", "falador_teleport"]);
    assert_eq!(debuff_spells(), vec!["confuse", "weaken", "curse", "bind"]);
}

/// Every D18 family finds its way: magic at the spell's level, the
/// spell's runes per cast; teleports and curses yield no object.
#[test]
fn every_spell_family_finds_its_way() {
    for (method, product, level) in [
        ("teleport:varrock_teleport", "", 25),
        ("teleport:lumbridge_teleport", "", 31),
        ("teleport:falador_teleport", "", 37),
        ("curse:confuse/chickens", "", 3),
        ("curse:weaken/cows", "", 11),
        ("curse:curse/cows", "", 19),
        ("curse:bind/cows", "", 20),
        ("bananas:bones", "banana", 15),
    ] {
        let (made, way) = method_way(method).unwrap_or_else(|| panic!("{method} has a way"));
        assert_eq!(made, product, "{method}");
        assert_eq!(way.skill, Some(("magic".to_string(), level)), "{method}");
        let spell = cast_spell(method).expect("casts a spell");
        for (rune, count) in crate::method_way::spell_runes(&spell) {
            assert!(way.inputs.contains(&(rune.clone(), count)), "{method} burns {count} {rune}: {:?}", way.inputs);
        }
    }
    assert!(method_way("curse:confuse/chickens").is_some_and(|(_, way)| !way.sites.is_empty()), "a curse names its camp");
    assert!(method_way("teleport:camelot_teleport").is_none(), "members teleport");
    assert!(method_way("curse:fire_strike/cows").is_none(), "a strike is no curse");
    assert!(method_way("curse:confuse/nowhere").is_none(), "unknown camp");
    assert!(method_way("bananas:big_bones").is_none(), "the spell converts plain bones only");
}

/// XP per CAST = the dump's tenths / 10 (unit S), whatever the cast yields.
#[test]
fn action_xp_is_per_cast() {
    for (method, tenths) in [
        ("teleport:varrock_teleport", 350),
        ("teleport:lumbridge_teleport", 410),
        ("teleport:falador_teleport", 480),
        ("curse:confuse/chickens", 130),
        ("curse:weaken/cows", 210),
        ("curse:curse/cows", 290),
        ("curse:bind/cows", 300),
        ("bananas:bones", 250),
    ] {
        let dump = lifescape_gamedata::spells::by_name(&cast_spell(method).expect("spell")).into_iter().next().expect("dump row").xp;
        assert_eq!(dump, tenths, "{method} dump row");
        assert_eq!(action_xp(method), Some(tenths as f64 / 10.0), "{method}");
        assert_eq!(cast_xp(method), Some(tenths as f64 / 10.0), "{method}");
        assert_eq!(xp_per_action(method, 40), Some(tenths as f64 / 10.0), "{method}: no damage term");
    }
}

/// A tranche burns the runes of every cast (and bananas a pack of bones
/// per cast): Varrock teleports 25 -> 30 at 35 XP a cast.
#[test]
fn tranches_size_the_runes_per_cast() {
    let casts = tranche_actions("teleport:varrock_teleport", 25, 30).expect("casts");
    assert_eq!(casts, (crate::sizing::xp_between(25, 30) / 35.0).ceil() as i64);
    assert_eq!(tranche_need("teleport:varrock_teleport", 25, 30, "lawrune"), Some(casts));
    assert_eq!(tranche_need("teleport:varrock_teleport", 25, 30, "airrune"), Some(3 * casts));
    let curses = tranche_actions("curse:confuse/chickens", 3, 10).expect("casts");
    let runes = tranche_consumption("curse:confuse/chickens", 3, 10);
    assert!(runes.contains(&("bodyrune".to_string(), curses)) && runes.contains(&("waterrune".to_string(), 3 * curses)), "{runes:?}");
    let bananas = tranche_actions("bananas:bones", 15, 20).expect("casts");
    assert_eq!(tranche_need("bananas:bones", 15, 20, "bones"), Some(bananas * BANANA_BONES_PER_CAST));
    assert_eq!(tranche_need("bananas:bones", 15, 20, "naturerune"), Some(bananas));
}

/// Crumble undead: a damage spell the ladder never casts on a cow, the
/// best spell at 39 on an all-undead camp (server `undead` param).
#[test]
fn crumble_undead_hits_the_undead_only() {
    assert_eq!(best_combat_spell(39), "fire_bolt", "any target: the elemental ladder");
    assert_eq!(best_combat_spell_at(39, "cows"), "fire_bolt");
    assert_eq!(best_combat_spell_at(39, "wild_skeletons"), "crumble_undead");
    assert_eq!(best_combat_spell_at(38, "wild_skeletons"), "fire_bolt", "not before 39");
    assert!(spell_fits_spot("crumble_undead", "skeletons") && spell_fits_spot("crumble_undead", "ghosts"));
    assert!(!spell_fits_spot("crumble_undead", "cows") && !spell_fits_spot("crumble_undead", "goblins"));
    assert_eq!(spell_spots("crumble_undead"), vec!["wild_skeletons"], "surface undead only: the sewers ride the ladder");
    let per_cast = xp_per_action("fight:magic/crumble_undead/wild_skeletons", 39).expect("xp");
    assert!(per_cast > 24.5, "base 24.5 plus damage: {per_cast}");
    assert!(tranche_consumption("fight:magic/crumble_undead/wild_skeletons", 39, 40).iter().any(|(rune, _)| rune == "chaosrune"));
}

/// The check list holds every D18 method once, each with its way.
#[test]
fn every_spell_method_is_listed() {
    let all = all_spell_methods();
    for method in ["teleport:varrock_teleport", "teleport:falador_teleport", "curse:confuse/chickens", "curse:bind/cows", "bananas:bones", "fight:magic/crumble_undead/wild_skeletons"] {
        assert!(all.contains(&method.to_string()), "{method} listed: {all:?}");
    }
    assert_eq!(curse_method("weaken", 11), "curse:weaken/cows");
    for method in all.iter().filter(|method| !method.starts_with("fight:")) {
        assert!(method_way(method).is_some(), "{method} has a way");
    }
}
