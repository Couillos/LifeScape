//! Loctype table with navigation overrides applied.
//!
//! Base rows come from the engine dump (`gametypes.json`); the overrides
//! file turns named decor into walls so the router detours instead of
//! pacing shut tiles (maze walls and chests: no override for loc 1530,
//! live shows normal doors there since 2026-11-06).

use lifescape_json::Value;

use crate::collision::LocDef;

/// True when this override value is a well-formed `[x, z, level]` tile.
fn is_tile(value: &Value) -> bool {
    value.as_arr().is_some_and(|items| items.len() == 3 && items.iter().all(|item| item.as_i64().is_some()))
}

/// Parse the loctype table and apply navigation overrides. Every
/// override entry needs a `reason` and a `verified` date; an override
/// pointing outside the loc table fails loud instead of inventing
/// a mapping. Entries scoped to one tile (`at`) are not stripped here:
/// the planner (`Tables::is_blocked`) reads those tiles as walls while
/// the same loc stays usable everywhere else.
pub fn parse_loctypes(gametypes_text: &str, overrides_text: &str) -> Result<Vec<Option<LocDef>>, String> {
    let gametypes = lifescape_json::parse(gametypes_text).map_err(|err| format!("parse gametypes: {err}"))?;
    let rows = gametypes.get("loc").as_arr().ok_or_else(|| "gametypes has no loc table".to_string())?;
    let overrides = lifescape_json::parse(overrides_text).map_err(|err| format!("parse nav_overrides: {err}"))?;
    if overrides.as_obj().is_none() {
        return Err("nav_overrides is not an object".to_string());
    }
    let mut blocked = Vec::new();
    match overrides.get("blocked_locs") {
        Value::Arr(entries) => {
            for (index, entry) in entries.iter().enumerate() {
                let reason = entry.get("reason").as_str().unwrap_or("");
                let verified = entry.get("verified").as_str().unwrap_or("");
                if reason.is_empty() || verified.is_empty() {
                    return Err(format!("nav_overrides blocked_locs[{index}] needs reason and verified"));
                }
                let loc = entry.get("loc").as_i64().ok_or_else(|| format!("nav_overrides blocked_locs[{index}] has no loc"))?;
                if loc < 0 {
                    return Err(format!("nav_overrides blocked_locs[{index}] has a negative loc"));
                }
                // One tile only: the planner blocks that tile, the loc
                // stays open everywhere else. Nothing to strip here.
                if !entry.get("at").is_null() {
                    if !is_tile(entry.get("at")) {
                        return Err(format!("nav_overrides blocked_locs[{index}] has a malformed at tile"));
                    }
                    continue;
                }
                blocked.push(loc as usize);
            }
        }
        value if value.is_null() => {}
        _ => return Err("nav_overrides blocked_locs is not an array".to_string()),
    }
    let mut maze_prefixes = Vec::new();
    match overrides.get("maze_locs") {
        Value::Arr(entries) => {
            for (index, entry) in entries.iter().enumerate() {
                let prefix = entry.get("name_prefix").as_str().unwrap_or("");
                let reason = entry.get("reason").as_str().unwrap_or("");
                let verified = entry.get("verified").as_str().unwrap_or("");
                if prefix.is_empty() || reason.is_empty() || verified.is_empty() {
                    return Err(format!("nav_overrides maze_locs[{index}] needs name_prefix, reason and verified"));
                }
                maze_prefixes.push(prefix.to_lowercase());
            }
        }
        value if value.is_null() => {}
        _ => return Err("nav_overrides maze_locs is not an array".to_string()),
    }
    let mut defs = Vec::with_capacity(rows.len());
    for (loc_id, row) in rows.iter().enumerate() {
        if row.is_null() {
            defs.push(None);
            continue;
        }
        let has_ops = row
            .get("ops")
            .as_arr()
            .is_some_and(|ops| ops.iter().any(|op| !op.is_null()));
        let name = row.get("name").as_str().unwrap_or("");
        let category = row.get("category").as_i64().unwrap_or(-1);
        let mut open_op = row
            .get("ops")
            .as_arr()
            .and_then(|ops| ops.iter().position(|op| {
                op.as_str().is_some_and(|name| name.eq_ignore_ascii_case("open"))
            }))
            .map(|index| index as u8 + 1);
        // Maze event walls advertise Open but the script refuses them,
        // except the swinging walllow doors (category 16). Maze chests
        // never open a path either. Prefixes come from nav_overrides.
        let lower = name.to_lowercase();
        if maze_prefixes.iter().any(|prefix| lower.starts_with(prefix)) && category != 16 {
            open_op = None;
        }
        if blocked.contains(&loc_id) {
            open_op = None;
        }
        let toll = row
            .get("name")
            .as_str()
            .is_some_and(|name| name.to_lowercase().contains("toll"));
        defs.push(Some(LocDef {
            blockwalk: row.get("blockwalk").as_bool().unwrap_or(false),
            blockrange: row.get("blockrange").as_bool().unwrap_or(false),
            length: row.get("length").as_i64().unwrap_or(1) as i32,
            width: row.get("width").as_i64().unwrap_or(1) as i32,
            active: row.get("active").as_i64().unwrap_or(0) as i32,
            has_ops,
            open_op,
            toll,
            gate_leaf: crate::collision::gate_leaf_of_category(category),
        }));
    }
    for loc in &blocked {
        if *loc >= defs.len() {
            return Err(format!("nav_overrides blocked loc {loc} is outside the loc table"));
        }
    }
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_match_previous_hardcodes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let gametypes_path = root.join("gamedata/data/gametypes.json");
        let overrides_path = root.join("gamedata/data/nav_overrides.json");
        if !gametypes_path.is_file() || !overrides_path.is_file() {
            return;
        }
        let gametypes = std::fs::read_to_string(&gametypes_path).expect("read gametypes");
        let overrides = std::fs::read_to_string(&overrides_path).expect("read overrides");
        let defs = parse_loctypes(&gametypes, &overrides).expect("parse loctypes");
        // Rampart 1530 is tile-scoped now: the loc keeps its op
        // everywhere, the planner walls off the one rampart tile.
        assert_eq!(defs[1530].as_ref().and_then(|def| def.open_op), Some(1));
        // Maze walls: stripped, except the swinging walllow doors (category 16).
        assert!(defs[3626].as_ref().is_some_and(|def| def.open_op.is_none()));
        assert_eq!(defs[3628].as_ref().and_then(|def| def.open_op), Some(1));
        assert_eq!(defs[3632].as_ref().and_then(|def| def.open_op), Some(1));
        // Maze chests: never a path.
        assert!(defs[3635].as_ref().is_some_and(|def| def.open_op.is_none()));
        // Untouched doors keep their op.
        assert_eq!(defs[3].as_ref().and_then(|def| def.open_op), Some(1));
        // Double gates: categories gate_main_closed / gate_outer_closed.
        use crate::collision::GateLeaf;
        assert_eq!(defs[1551].as_ref().and_then(|def| def.gate_leaf), Some(GateLeaf::Main));
        assert_eq!(defs[1553].as_ref().and_then(|def| def.gate_leaf), Some(GateLeaf::Outer));
        assert_eq!(defs[1552].as_ref().and_then(|def| def.gate_leaf), None);
        assert_eq!(defs[3].as_ref().and_then(|def| def.gate_leaf), None);
        // Single doors: category door_closed (Port Sarim cook house 1530).
        assert_eq!(defs[1530].as_ref().and_then(|def| def.gate_leaf), Some(GateLeaf::Door));
        assert_eq!(defs[1531].as_ref().and_then(|def| def.gate_leaf), None);
    }
}
