//! Food healing values.

use std::sync::OnceLock;

use crate::{Json, table_value};

fn foods() -> &'static Vec<(String, i64)> {
    static CACHE: OnceLock<Vec<(String, i64)>> = OnceLock::new();
    CACHE.get_or_init(|| match table_value("foods") {
        Some(Json::Obj(fields)) => {
            fields.iter().filter_map(|(name, value)| value.as_i64().map(|heal| (name.clone(), heal))).collect()
        }
        _ => Vec::new(),
    })
}

/// Every food and its hitpoints restored.
pub fn all_foods() -> Vec<(String, i64)> {
    foods().clone()
}

/// Hitpoints restored by a food, or 0 when it is not edible.
pub fn heal(name: &str) -> i64 {
    foods().iter().find(|(entry, _)| entry == name).map(|(_, heal)| *heal).unwrap_or(0)
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
}
