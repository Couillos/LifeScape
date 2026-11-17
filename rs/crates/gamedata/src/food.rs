//! Food healing values.

mod generated {
    include!(concat!(env!("OUT_DIR"), "/foods_generated.rs"));
}

/// Internal food key for a live name: the table keys are internal
/// ("shrimp") but every live caller passes obj display names ("Shrimps").
/// The build-time index maps both spellings, first dump row winning.
fn key_for(name: &str) -> Option<&'static str> {
    let wanted = name.to_lowercase();
    generated::ALIASES
        .binary_search_by(|entry| entry.0.cmp(wanted.as_str()))
        .ok()
        .map(|position| generated::ALIASES[position].1)
}

/// The server's `alcoholic_drinks` category (`cooking.obj`, `wine.obj`,
/// `cocktails.obj`, `viking.obj`): every one lowers Attack when drunk
/// (`consume_effects.dbrow`: beer heals 1, Attack -1 and -6 %, Strength
/// +1; wine heals 11, Attack -2). User rule 2026-11-17: beer is never
/// food — the whole class is left out, a fight never drinks it, the
/// food counts and the food demand never read it (live: 1 478 beers
/// "eaten" in ladder fights, the foodless rule never fired).
pub const ALCOHOLIC_DRINKS: [&str; 16] = [
    "asgarnian_ale",
    "wizards_mind_bomb",
    "greenmans_ale",
    "dragon_bitter",
    "dwarven_stout",
    "grog",
    "beer",
    "moonlight_mead",
    "jug_bad_wine",
    "jug_wine",
    "half_full_wine_jug",
    "vodka",
    "whisky",
    "gin",
    "brandy",
    "viking_tankard_full",
];

/// True for an alcoholic drink (internal key or display name).
pub fn is_alcoholic(name: &str) -> bool {
    let key = key_for(name).unwrap_or(name);
    ALCOHOLIC_DRINKS.contains(&key)
}

/// Every food and its hitpoints restored (alcoholic drinks left out).
pub fn all_foods() -> Vec<(String, i64)> {
    generated::ALL.iter().filter(|(name, _)| !ALCOHOLIC_DRINKS.contains(name)).map(|(name, heal)| ((*name).to_string(), *heal)).collect()
}

/// Hitpoints restored by a food, or 0 when it is not edible (alcoholic
/// drinks included: never food).
/// Accepts internal keys ("shrimp") and display names ("Shrimps").
pub fn heal(name: &str) -> i64 {
    let key = key_for(name).unwrap_or(name);
    if ALCOHOLIC_DRINKS.contains(&key) {
        return 0;
    }
    generated::ALL.binary_search_by(|entry| entry.0.cmp(key)).ok().map(|position| generated::ALL[position].1).unwrap_or(0)
}

/// Highest-healing food among the given names, or None.
pub fn best_food(names: &[&str]) -> Option<(String, i64)> {
    let mut best: Option<(String, i64)> = None;
    for name in names {
        let amount = heal(name);
        if amount > 0 && best.as_ref().is_none_or(|(_, heal)| amount > *heal) {
            best = Some(((*name).to_string(), amount));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{best_food, heal};

    #[test]
    fn healing_values() {
        // shark heals 20, unknown heals 0,
        // best of trout/shark/lobster is shark.
        assert_eq!(heal("shark"), 20);
        assert_eq!(heal("nope"), 0);
        assert_eq!(best_food(&["trout", "shark", "lobster"]), Some(("shark".to_string(), 20)));
        assert!(best_food(&["nope"]).is_none());
        // 139 dump rows minus the 15 alcoholic drinks the dump heals
        // (bad wine heals nothing, absent from the dump).
        assert_eq!(super::all_foods().len(), 124);
    }

    /// User rule 2026-11-17: beer is never food, nor any drink of the
    /// server's `alcoholic_drinks` category (each one lowers Attack).
    #[test]
    fn alcoholic_drinks_are_never_food() {
        for drink in ["beer", "Beer", "asgarnian_ale", "Jug of wine", "wizards_mind_bomb", "dwarven_stout"] {
            assert_eq!(heal(drink), 0, "{drink} is no food");
            assert!(super::is_alcoholic(drink), "{drink}");
        }
        assert!(!super::all_foods().iter().any(|(name, _)| name == "beer"));
        assert_eq!(best_food(&["Beer", "Cabbage"]).map(|(name, _)| name), Some("Cabbage".to_string()));
        assert!(best_food(&["Beer"]).is_none());
        assert!(!super::is_alcoholic("shrimp"));
        // Every listed drink but bad wine (heals nothing) sits in the
        // heal dump: no stale key.
        for drink in super::ALCOHOLIC_DRINKS.iter().filter(|drink| **drink != "jug_bad_wine") {
            assert!(super::generated::ALL.iter().any(|(name, _)| name == drink), "{drink} in foods.json");
        }
    }

    #[test]
    fn display_names_heal() {
        // Live callers pass obj display names, never internal keys.
        assert!(heal("Shrimps") > 0);
        assert_eq!(heal("Bread"), heal("bread"));
        assert_eq!(heal("Cooked meat"), heal("cooked_meat"));
        assert_eq!(best_food(&["Shrimps", "Bread"]).map(|(name, _)| name), Some("Bread".to_string()));
        assert_eq!(heal("nope"), 0);
        // Non-food must terminate (regression: mutual recursion with the
        // display index overflowed the stack and aborted the worker).
        assert_eq!(heal("Bronze sword"), 0);
        assert_eq!(heal("Cowhide"), 0);
    }
}
