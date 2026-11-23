#![forbid(unsafe_code)]
//! Object names between the reports (display names: `Raw shrimps`) and
//! the game data (debug names: `raw_shrimp`), for the rates.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Display name of a debug name (itself when the dump has none).
pub fn display_of(debug: &str) -> String {
    // Static dump, asked per flow of every yield read (the guides read
    // ~50 000 a set): once per name.
    static MEMO: OnceLock<std::sync::Mutex<HashMap<String, String>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(debug) {
        return known.clone();
    }
    let found = lifescape_gamedata::objs_by_name(debug).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(debug).to_string();
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(debug.to_string(), found.clone());
    found
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

/// Debug name of a display name in the flows of `method`, on the side
/// the flow goes: an object going OUT (`consumed`: an input, a
/// consumable) is one the method's way takes (inputs, then tools), one
/// coming IN its product; else `debug_of`. Several objects share a
/// display (live 2026-11-16: "Sapphire amulet" is the strung amulet
/// `enchant:strung_sapphire_amulet` takes AND the unstrung one
/// `string:` takes to make it).
pub fn debug_in(method: &str, display: &str, consumed: bool) -> String {
    // Static dump and ways: once per (method, display, side) (a way
    // carries its sites, copied per read without this).
    static MEMO: OnceLock<std::sync::Mutex<HashMap<(String, String, bool), String>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let key = (method.to_string(), display.to_string(), consumed);
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        return known.clone();
    }
    let found = compute_debug_in(method, display, consumed);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key, found.clone());
    found
}

fn compute_debug_in(method: &str, display: &str, consumed: bool) -> String {
    let wanted = display.to_lowercase();
    if let Some((product, way)) = lifescape_planner::method_way::method_way(method) {
        let own: Vec<String> = if consumed { way.inputs.into_iter().map(|(input, _)| input).chain(way.tools).collect() } else { vec![product] };
        for name in own {
            if lifescape_gamedata::objs_by_name(&name).iter().any(|obj| obj.display.unwrap_or(&name).to_lowercase() == wanted) {
                return name;
            }
        }
    }
    debug_of(display)
}

/// `units_in_debug` in the names of `method`'s own objects (`debug_in`:
/// an `input` unit is consumed, a `product` one made).
pub fn units_in_method(method: &str, units: &[(String, String, f64)]) -> Vec<(String, String, f64)> {
    units
        .iter()
        .map(|(kind, subject, count)| {
            let subject = if matches!(kind.as_str(), "product" | "input") { debug_in(method, subject, kind == "input") } else { subject.clone() };
            (kind.clone(), subject, *count)
        })
        .collect()
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
