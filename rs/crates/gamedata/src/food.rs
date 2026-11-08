//! Food healing values.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::{Json, table, table_value};

fn foods() -> &'static Vec<(String, i64)> {
    static CACHE: OnceLock<Vec<(String, i64)>> = OnceLock::new();
    CACHE.get_or_init(|| match table_value("foods") {
        Some(Json::Obj(fields)) => {
            fields.iter().filter_map(|(name, value)| value.as_i64().map(|heal| (name.clone(), heal))).collect()
        }
        _ => Vec::new(),
    })
}

/// Internal food key for a live name: the table keys are internal
/// ("shrimp") but every live caller passes obj display names ("Shrimps"),
/// so exact matching heals nothing (t02 live: never ate, never stocked).
fn key_for(name: &str) -> Option<String> {
    static CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();
    let index = CACHE.get_or_init(|| {
        let mut map = HashMap::new();
        for row in table("objs") {
            let Some(internal) = row.get("name").as_str() else {
                continue;
            };
            map.entry(internal.to_lowercase()).or_insert(internal.to_string());
            if let Some(display) = row.get("display").as_str() {
                map.entry(display.to_lowercase()).or_insert(internal.to_string());
            }
        }
        map
    });
    index.get(&name.to_lowercase()).cloned()
}

/// Every food and its hitpoints restored.
pub fn all_foods() -> Vec<(String, i64)> {
    foods().clone()
}

/// Hitpoints restored by a food, or 0 when it is not edible.
/// Accepts internal keys ("shrimp") and display names ("Shrimps").
pub fn heal(name: &str) -> i64 {
    let key = key_for(name).unwrap_or_else(|| name.to_string());
    foods().iter().find(|(entry, _)| *entry == key).map(|(_, heal)| *heal).unwrap_or(0)
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
