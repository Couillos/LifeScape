#![forbid(unsafe_code)]
//! Object names between the reports (display names: `Raw shrimps`) and
//! the game data (debug names: `raw_shrimp`), for the rates.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Display name of a debug name (itself when the dump has none).
pub fn display_of(debug: &str) -> String {
    lifescape_gamedata::objs_by_name(debug).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(debug).to_string()
}

/// Debug name of a display name: the object named like its display
/// first (`coins` over `fake_coins`), banknotes never; itself when the
/// dump knows no such display.
pub fn debug_of(display: &str) -> String {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        for obj in lifescape_gamedata::all_objs() {
            let (Some(name), Some(shown)) = (obj.name, obj.display) else { continue };
            if name.starts_with("cert_") {
                continue;
            }
            let key = shown.to_lowercase();
            let exact = key.replace(' ', "_") == name;
            if exact || !out.contains_key(&key) {
                out.insert(key, name.to_string());
            }
        }
        out
    });
    table.get(&display.to_lowercase()).cloned().unwrap_or_else(|| display.to_string())
}

/// Units with their object subjects in debug names (spells and NPCs are
/// keyed by their game names already).
pub fn units_in_debug(units: &[(String, String, f64)]) -> Vec<(String, String, f64)> {
    units
        .iter()
        .map(|(kind, subject, count)| {
            let subject = if matches!(kind.as_str(), "product" | "input") { debug_of(subject) } else { subject.clone() };
            (kind.clone(), subject, *count)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_round_trip() {
        assert_eq!(super::debug_of("Raw shrimps"), "raw_shrimp");
        assert_eq!(super::display_of("raw_shrimp"), "Raw shrimps");
        assert_eq!(super::debug_of("Coins"), "coins");
    }
}
