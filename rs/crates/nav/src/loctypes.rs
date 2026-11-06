//! Loctype table with navigation overrides applied.
//!
//! Base rows come from the engine dump (`gametypes.json`); the overrides
//! file turns named decor into walls so the router detours instead of
//! pacing shut tiles (rampart 1530, maze walls and chests).

use lifescape_json::Value;

use crate::collision::LocDef;

/// Parse the loctype table and apply navigation overrides. Every
/// override entry needs a `reason` and a `verified` date; an override
/// pointing outside the loc table fails loud instead of inventing
/// a mapping.
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
        }));
    }
    for loc in &blocked {
        if *loc >= defs.len() {
            return Err(format!("nav_overrides blocked loc {loc} is outside the loc table"));
        }
    }
    Ok(defs)
}
