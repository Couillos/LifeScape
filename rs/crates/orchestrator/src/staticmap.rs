//! Static scenery for the live session, read from the server dumps.
//!
//! Map squares load lazily around the bot instead of the whole world at
//! open: the zip index plus loc defs builds in a fraction of a second,
//! each 64x64 square paints on first touch. Every session reads straight
//! out of the pack file, so the OS page cache shares the bytes across all
//! workers: the 14 MB pack lives on disk once, not once per client.
//! Exactly like the game server's own data (lostcity is the only source
//! of truth; the legacy Python loader is not a reference).

use std::path::Path;

use lifescape_sdk::collision::{LocDef, MapPack, zip_spans};

use crate::json::Value;
use crate::paths;

/// Loc defs for every loc id in the engine dump (`gamedata/data/gametypes.json`).
/// Null rows stay None; the map loader skips locs without a def.
pub(crate) fn load_loctypes(gametypes_path: &Path) -> Result<Vec<Option<LocDef>>, String> {
    let text = std::fs::read_to_string(gametypes_path).map_err(|err| format!("read {}: {err}", gametypes_path.display()))?;
    let parsed = crate::json::parse(&text).map_err(|err| format!("parse {}: {err}", gametypes_path.display()))?;
    let rows = parsed.get("loc").as_arr().ok_or_else(|| format!("{} has no loc table", gametypes_path.display()))?;
    let mut defs = Vec::with_capacity(rows.len());
    for (loc_id, row) in rows.iter().enumerate() {
        if matches!(row, Value::Null) {
            defs.push(None);
            continue;
        }
        let has_ops = row
            .get("ops")
            .as_arr()
            .is_some_and(|ops| ops.iter().any(|op| !matches!(op, Value::Null)));
        let name = row.get("name").as_str().unwrap_or("");
        let category = row.get("category").as_i64().unwrap_or(-1);
        let mut open_op = row
            .get("ops")
            .as_arr()
            .and_then(|ops| ops.iter().position(|op| {
                op.as_str().is_some_and(|name| name.eq_ignore_ascii_case("open"))
            }))
            .map(|index| index as u8 + 1);
        // Maze routing exceptions (server scripts are the truth):
        // - maze walls outside category 16 advertise Open but the script
        //   answers "I can't open that" (macro_event_maze.rs2); only
        //   category 16 (macro_maze_wall_door: walllow, safe1..safe4)
        //   swings. Checked against gametypes.json: 3628..3632.
        // - maze chests advertise Open but looting one never lets anyone
        //   pass, so they must read as walls for planning.
        let lower = name.to_lowercase();
        let maze_wall = lower.starts_with("macro_maze_wall") || lower.starts_with("macor_maze_wall");
        if (maze_wall && category != 16) || lower.starts_with("macro_maze_chest") {
            open_op = None;
        }
        // Rampart "door" 1530 advertises Open and flips live, but the server
        // refuses every eastward step through it (verified live, repeatedly,
        // across sessions): the castle ramparts have no gate there, only
        // decor. Read it as a wall like maze chests, so the router detours
        // instead of pacing a tile that never opens. If a real 1530 passage
        // is ever needed, it fails loud (Unreachable + blockers: see the
        // walker's no-route report), never silent.
        if loc_id == 1530 {
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
    Ok(defs)
}

/// Transition rows for stair/ladder rides (`gamedata/data/transitions.json`).
/// Parsed once per session; the file is small and static.
pub fn load_transitions() -> Result<Vec<lifescape_sdk::TransitionRow>, String> {
    let path = paths::root().join("gamedata").join("data").join("transitions.json");
    let text = std::fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let parsed = crate::json::parse(&text).map_err(|err| format!("parse {}: {err}", path.display()))?;
    let rows = parsed.as_arr().ok_or_else(|| format!("{} is not an array", path.display()))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let triple = |key: &str| -> Option<(i32, i32, i32)> {
            let items = row.get(key).as_arr()?;
            if items.len() != 3 {
                return None;
            }
            Some((items[0].as_i64()? as i32, items[1].as_i64()? as i32, items[2].as_i64()? as i32))
        };
        let (Some(from_tile), Some(to_tile)) = (triple("from"), triple("to")) else {
            continue;
        };
        let footprint: Vec<(i32, i32)> = row
            .get("footprint")
            .as_arr()
            .map(|tiles| {
                tiles
                    .iter()
                    .filter_map(|tile| {
                        let pair = tile.as_arr()?;
                        Some((pair.first()?.as_i64()? as i32, pair.get(1)?.as_i64()? as i32))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(lifescape_sdk::TransitionRow {
            name: row.get("name").as_str().unwrap_or("").to_string(),
            from_tile,
            to_tile,
            loc: row.get("climb_loc").as_i64().unwrap_or(0) as u16,
            op: row.get("climb_op").as_i64().unwrap_or(1) as u8,
            footprint,
            disabled: row.get("disabled").as_bool().unwrap_or(false),
        });
    }
    Ok(out)
}

/// Build the pack index (zip spans plus loc defs) without painting any
/// square: fast enough to run on every session open. Squares paint on
/// demand through `Bot::ensure_area`.
pub fn build_pack(engine_dir: &str) -> Result<MapPack, String> {
    let pack_dir = Path::new(engine_dir).join("data").join("pack");
    let _timer = if std::env::var("MAP_DEBUG").is_ok() { Some(std::time::Instant::now()) } else { None };
    let gametypes = paths::root().join("gamedata").join("data").join("gametypes.json");
    let defs = load_loctypes(&gametypes)?;
    let zipped = pack_dir.join(".cache").join("maps-server.zip");
    if !zipped.is_file() {
        return Err(format!("no maps at {}", zipped.display()));
    }
    if let Some(timer) = _timer { eprintln!("gametypes parsed in {:?}", timer.elapsed()); }
    let timer = std::time::Instant::now();
    let spans = zip_spans(&zipped).map_err(|err| format!("index {}: {err}", zipped.display()))?;
    if std::env::var("MAP_DEBUG").is_ok() { eprintln!("zip indexed in {:?}", timer.elapsed()); }
    Ok(MapPack::new(zipped, spans, defs))
}

#[cfg(test)]
mod tests {
    use super::load_loctypes;
    use crate::paths;

    #[test]
    fn bankbooth_def_matches_server_dump() {
        let path = paths::root().join("gamedata").join("data").join("gametypes.json");
        let defs = load_loctypes(&path).expect("gametypes parse");
        assert!(defs.len() >= 2214, "loc table holds the bankbooth");
        let booth = defs[2213].as_ref().expect("bankbooth def");
        assert!(booth.blockwalk, "booths block walking per the dump");
        assert!(booth.has_ops, "booths expose Use ops per the dump");
    }

    #[test]
    fn pack_index_lists_squares() {
        let root = paths::root();
        let pack = super::build_pack(&root.join("../lostcity/engine").to_string_lossy())
            .expect("pack index builds");
        assert!(pack.square_count() > 100, "the server ships hundreds of squares");
    }
    #[test]
    fn mainland_bank_routes() {
        use lifescape_sdk::collision::{CollisionMap, paint_square, zip_index, zip_slice};
        let root = paths::root();
        let pack_dir = root.join("../lostcity/engine/data/pack");
        let bytes = std::fs::read(pack_dir.join(".cache").join("maps-server.zip")).unwrap();
        let index = zip_index(&bytes).unwrap();
        let defs = load_loctypes(&root.join("gamedata").join("data").join("gametypes.json")).unwrap();
        let mut collision = CollisionMap::new();
        let mut locs = std::collections::HashMap::new();
        for (mx, mz) in [(47, 52), (48, 50), (48, 51)] {
            let g = index.get(&format!("m{mx}_{mz}")).unwrap();
            let l = index.get(&format!("l{mx}_{mz}")).unwrap();
            let mut actions = std::collections::HashMap::new();
            let mut layers = std::collections::HashMap::new();
            paint_square(Some(&mut collision), zip_slice(&bytes, *g), zip_slice(&bytes, *l),
                mx * 64, mz * 64, &defs, &mut locs, &mut actions, &mut layers).unwrap();
        }
        assert!(locs.values().any(|(id, _, _)| *id == 2213), "Falador booths load");
        let mut router = lifescape_sdk::route::Router::new();
        let bank = router.find_path_to_tile(&collision, 0, 3091, 3230, 3091, 3240);
        assert!(!bank.is_empty(), "open ground near Lumbridge bank routes");
    }


}
