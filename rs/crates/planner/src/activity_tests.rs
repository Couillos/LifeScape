use super::{activity_of_method, activity_of_run, fishing_activity};

fn no_units() -> Vec<(String, String, i64)> {
    Vec::new()
}

/// A fight is keyed by its spot and style, never by the object wanted:
/// feathers, arrows or nature runes farmed on the same goblins are one
/// activity.
#[test]
fn a_fight_is_keyed_by_its_spot_not_the_wanted_object() {
    assert_eq!(activity_of_run("farm:naturerune", "melee:attack", "goblins", &no_units()), "fight:melee/goblins");
    assert_eq!(activity_of_run("farm:bronze_arrow", "melee:attack", "goblins", &no_units()), "fight:melee/goblins");
    assert_eq!(activity_of_run("farm:feather", "melee:controlled", "chickens", &no_units()), "fight:melee/chickens");
    // The planner's fight method lands on the same key.
    assert_eq!(activity_of_run("fight:melee/goblins", "melee:attack", "goblins", &no_units()), "fight:melee/goblins");
    // No target: the spot the planner sends that drop farm to.
    assert_eq!(activity_of_run("farm:feather", "melee:attack", "", &no_units()), "fight:melee/chickens");
    assert_eq!(activity_of_method("farm:feather"), "fight:melee/chickens");
}

/// Ranged and magic drop farms key their own style; a magic run names
/// the spell it cast, never a guessed one.
#[test]
fn ranged_and_magic_fights_key_their_style() {
    assert_eq!(activity_of_run("farm:feather", "ranged", "chickens", &no_units()), "fight:ranged/chickens");
    let casts = vec![("cast".to_string(), "wind_strike".to_string(), 12)];
    assert_eq!(activity_of_run("farm:feather", "magic", "chickens", &casts), "fight:magic/wind_strike/chickens");
    assert_eq!(activity_of_run("farm:feather", "magic", "chickens", &no_units()), "farm:feather", "no spell seen: method kept");
}

/// One fishing spot option is one activity, whatever catch was wanted.
#[test]
fn fishing_is_keyed_by_its_spot_option() {
    assert_eq!(fishing_activity("raw_salmon").as_deref(), Some("gather:fishing/raw_trout"));
    assert_eq!(activity_of_method("gather:fishing/raw_salmon"), "gather:fishing/raw_trout");
    assert_eq!(activity_of_method("farm:raw_salmon"), "gather:fishing/raw_trout");
    assert_eq!(activity_of_method("farm:raw_anchovies"), "gather:fishing/raw_shrimp");
    assert_eq!(activity_of_method("gather:fishing/raw_pike"), "gather:fishing/raw_pike");
}

/// Gather legs land on the gatherer's key; recipes, pickups and the rest
/// already name their activity.
#[test]
fn gathers_and_recipes_keep_one_key() {
    assert_eq!(activity_of_method("farm:coal"), "gather:mining/coal");
    assert_eq!(activity_of_method("farm:clay"), "gather:mining/clay");
    assert_eq!(activity_of_method("farm:yew_logs"), "gather:woodcut/yew_logs");
    assert_eq!(activity_of_method("farm:logs"), "gather:woodcut/logs");
    assert_eq!(activity_of_method("farm:blankrune"), "essence:blankrune");
    assert_eq!(activity_of_method("cook/raw_trout"), "cook:trout", "one dish, one activity");
    for kept in ["smelt:steel_bar", "smith:iron_platebody", "ground:airrune", "pick:wheat", "curse:weaken/cows", "fight:melee/ladder", "cook:swordfish"] {
        assert_eq!(activity_of_method(kept), kept);
    }
}
