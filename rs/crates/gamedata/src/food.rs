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

/// Every food and its hitpoints restored.
pub fn all_foods() -> Vec<(String, i64)> {
    generated::ALL.iter().map(|(name, heal)| ((*name).to_string(), *heal)).collect()
}

/// Hitpoints restored by a food, or 0 when it is not edible.
/// Accepts internal keys ("shrimp") and display names ("Shrimps").
pub fn heal(name: &str) -> i64 {
    let key = key_for(name).unwrap_or(name);
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
        assert_eq!(super::all_foods().len(), 139);
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
