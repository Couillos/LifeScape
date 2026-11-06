//! Typed navigation tables: transitions, prober verdicts, door rules.
//!
//! Everything here is data loaded from `gamedata/data/`: the transition
//! dump, the optional prober verdicts, and the hand-checked door rules.
//! No network, no live state. Tag vocabulary for `requires` and
//! `satisfied`: `quest:<name>`, `item:<id>`, `coins:<n>`, `qp:<n>`
//! (quest-point total, as checked by the content scripts).

use std::collections::HashMap;
use std::path::Path;

use lifescape_json::Value;

use crate::plan::Tile;

/// One ride row: a stair/ladder/trapdoor tile, where it takes you, and
/// which loc op rides it. `from` is the table tile, `to` the explicit
/// arrival, `delta` the stand-relative arrival when set.
#[derive(Clone, Debug)]
pub struct Transition {
    pub name: String,
    pub from: Tile,
    pub to: Tile,
    pub loc: u16,
    pub climb_loc: u16,
    pub climb_op: u8,
    pub footprint: Vec<(i32, i32)>,
    pub delta: Option<(i32, i32, i32)>,
    pub open_to: Option<u16>,
    pub disabled: bool,
}

/// What the prober saw when it rode this transition live.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Ok,
    NeedsChoice,
    WrongDestination,
    NoMove,
}

impl Verdict {
    /// Failed rides never plan: the destination is known wrong.
    pub fn excludes(self) -> bool {
        matches!(self, Verdict::WrongDestination | Verdict::NoMove)
    }

    /// Seen working rides plan without the unverified penalty.
    pub fn verified(self) -> bool {
        matches!(self, Verdict::Ok | Verdict::NeedsChoice)
    }
}

/// A door that only opens with tags in hand (`coins:10`, `qp:32`...).
/// `at` is the door tile, `loc` 0 matches any door there.
#[derive(Clone, Debug)]
pub struct Requirement {
    pub loc: u16,
    pub at: Tile,
    pub requires: Vec<String>,
    pub reason: String,
}

/// A door that only swings from one side. No entries ship yet.
#[derive(Clone, Debug)]
pub struct OneWay {
    pub loc: u16,
    pub at: Tile,
    pub passable_from: Side,
    pub reason: String,
}

/// North/south/east/west side of a door tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    North,
    South,
    East,
    West,
}

impl Side {
    fn parse(raw: &str) -> Option<Side> {
        match raw.to_lowercase().as_str() {
            "north" => Some(Side::North),
            "south" => Some(Side::South),
            "east" => Some(Side::East),
            "west" => Some(Side::West),
            _ => None,
        }
    }

    /// True when this stand tile sits on the named side of the door.
    pub fn holds(self, stand: Tile, door: Tile) -> bool {
        match self {
            Side::North => stand.z > door.z,
            Side::South => stand.z < door.z,
            Side::East => stand.x > door.x,
            Side::West => stand.x < door.x,
        }
    }
}

/// A door tile that never swings (rampart decor with no real passage).
/// `loc` 0 matches any door on that tile. Entries without a tile are
/// handled by the loctype table instead (the whole loc loses its op).
#[derive(Clone, Debug)]
pub struct BlockedDoor {
    pub loc: u16,
    pub at: Tile,
    pub reason: String,
}

/// A hand-added edge (pushed secret wall...). None ship yet.
#[derive(Clone, Debug)]
pub struct CustomEdge {
    pub name: String,
    pub loc: u16,
    pub op: u8,
    pub stand: Tile,
    pub to: Tile,
    pub requires: Vec<String>,
    pub reason: String,
}

/// Every table `plan` consults, loaded from one data directory.
pub struct Tables {
    pub transitions: Vec<Transition>,
    pub verified: HashMap<(u16, i32, i32, i32), Verdict>,
    pub verified_any_level: Vec<((u16, i32, i32), Verdict)>,
    pub requirements: Vec<Requirement>,
    pub one_way: Vec<OneWay>,
    pub customs: Vec<CustomEdge>,
    pub blocked: Vec<BlockedDoor>,
}

impl Tables {
    /// Load `transitions.json` plus `nav_overrides.json` from `data_dir`;
    /// `transitions_verified.json` is optional (absent means all
    /// transitions plan with the unverified penalty).
    pub fn load(data_dir: &Path) -> Result<Tables, String> {
        let transitions = load_transitions(&data_dir.join("transitions.json"))?;
        let (verified, verified_any_level) = load_verified(&data_dir.join("transitions_verified.json"));
        let (requirements, one_way, customs, blocked) = load_overrides(&data_dir.join("nav_overrides.json"))?;
        Ok(Tables { transitions, verified, verified_any_level, requirements, one_way, customs, blocked })
    }

    /// Prober verdict for one transition, if it ever rode it.
    pub fn verdict(&self, loc: u16, from: Tile) -> Option<Verdict> {
        if let Some(seen) = self.verified.get(&(loc, from.level, from.x, from.z)) {
            return Some(*seen);
        }
        self.verified_any_level.iter().find(|((id, x, z), _)| *id == loc && *x == from.x && *z == from.z).map(|(_, seen)| *seen)
    }

    /// Missing tags for a door tile, empty when it swings freely.
    pub fn missing(&self, loc: u16, at: Tile, satisfied: &std::collections::HashSet<String>) -> Vec<String> {
        let mut missing = Vec::new();
        for entry in &self.requirements {
            if entry.at != at || (entry.loc != 0 && entry.loc != loc) {
                continue;
            }
            for tag in &entry.requires {
                if !satisfied.contains(tag) && !missing.contains(tag) {
                    missing.push(tag.clone());
                }
            }
        }
        missing
    }

    /// True when this stand may cross this door tile.
    pub fn side_ok(&self, loc: u16, at: Tile, stand: Tile) -> bool {
        for entry in &self.one_way {
            if entry.at == at && (entry.loc == 0 || entry.loc == loc) && !entry.passable_from.holds(stand, at) {
                return false;
            }
        }
        true
    }

    /// True when this door tile never swings: the planner reads it as
    /// wall even though the loc advertises Open elsewhere.
    pub fn is_blocked(&self, loc: u16, at: Tile) -> bool {
        self.blocked.iter().any(|entry| entry.at == at && (entry.loc == 0 || entry.loc == loc))
    }
}

fn tile_of(items: &[Value]) -> Option<Tile> {
    if items.len() != 3 {
        return None;
    }
    Some(Tile { level: items[0].as_i64()? as i32, x: items[1].as_i64()? as i32, z: items[2].as_i64()? as i32 })
}

fn triple(items: &[Value]) -> Option<(i32, i32, i32)> {
    if items.len() != 3 {
        return None;
    }
    Some((items[0].as_i64()? as i32, items[1].as_i64()? as i32, items[2].as_i64()? as i32))
}

fn load_transitions(path: &Path) -> Result<Vec<Transition>, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let parsed = lifescape_json::parse(&text).map_err(|err| format!("parse {}: {err}", path.display()))?;
    let rows = parsed.as_arr().ok_or_else(|| format!("{} is not an array", path.display()))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let (Some(from), Some(to)) = (row.get("from").as_arr().and_then(|items| tile_of(items)), row.get("to").as_arr().and_then(|items| tile_of(items))) else {
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
        let disabled = match row.get("disabled") {
            Value::Str(text) => !text.is_empty(),
            Value::Bool(flag) => *flag,
            Value::Int(num) => *num != 0,
            _ => false,
        };
        out.push(Transition {
            name: row.get("name").as_str().unwrap_or("").to_string(),
            from,
            to,
            loc: row.get("loc").as_i64().unwrap_or(0) as u16,
            climb_loc: row.get("climb_loc").as_i64().unwrap_or(0) as u16,
            climb_op: row.get("climb_op").as_i64().unwrap_or(1) as u8,
            footprint,
            delta: row.get("delta").as_arr().and_then(|items| triple(items)),
            open_to: row.get("open_to").as_i64().map(|num| num as u16),
            disabled,
        });
    }
    Ok(out)
}

fn load_verified(path: &Path) -> (HashMap<(u16, i32, i32, i32), Verdict>, Vec<((u16, i32, i32), Verdict)>) {
    let mut exact = HashMap::new();
    let mut loose = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return (exact, loose);
    };
    let Ok(parsed) = lifescape_json::parse(&text) else {
        return (exact, loose);
    };
    let rows = parsed.as_arr().cloned().unwrap_or_default();
    for row in &rows {
        let loc = row.get("loc").as_i64().unwrap_or(0) as u16;
        let verdict = match row.get("result").as_str().unwrap_or("") {
            "ok" => Verdict::Ok,
            "needs_choice" => Verdict::NeedsChoice,
            "wrong_destination" => Verdict::WrongDestination,
            "no_move" => Verdict::NoMove,
            _ => continue,
        };
        let items = row.get("from").as_arr().cloned().unwrap_or_default();
        if items.len() == 3 {
            if let (Some(level), Some(x), Some(z)) = (items[0].as_i64(), items[1].as_i64(), items[2].as_i64()) {
                exact.insert((loc, level as i32, x as i32, z as i32), verdict);
            }
        } else if items.len() == 2 {
            if let (Some(x), Some(z)) = (items[0].as_i64(), items[1].as_i64()) {
                loose.push(((loc, x as i32, z as i32), verdict));
            }
        }
    }
    (exact, loose)
}

fn parse_tile(value: &Value) -> Option<Tile> {
    let items = value.as_arr()?;
    if items.len() != 3 {
        return None;
    }
    Some(Tile { x: items[0].as_i64()? as i32, z: items[1].as_i64()? as i32, level: items[2].as_i64()? as i32 })
}

fn checked_entry<'a>(rows: &'a [Value], key: &str, index: usize) -> Result<&'a Value, String> {
    rows.get(index).ok_or_else(|| format!("nav_overrides {key}[{index}] is missing"))
}

fn load_overrides(path: &Path) -> Result<(Vec<Requirement>, Vec<OneWay>, Vec<CustomEdge>, Vec<BlockedDoor>), String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let parsed = lifescape_json::parse(&text).map_err(|err| format!("parse {}: {err}", path.display()))?;
    if parsed.as_obj().is_none() {
        return Err("nav_overrides is not an object".to_string());
    }
    let mut requirements = Vec::new();
    let rows = parsed.get("requirements").as_arr().cloned().unwrap_or_default();
    for (index, row) in rows.iter().enumerate() {
        let entry = checked_entry(&rows, "requirements", index)?;
        let _ = entry;
        let reason = row.get("reason").as_str().unwrap_or("");
        if reason.is_empty() {
            return Err(format!("nav_overrides requirements[{index}] needs a reason"));
        }
        let Some(at) = parse_tile(row.get("at")) else {
            return Err(format!("nav_overrides requirements[{index}] has no at tile"));
        };
        let tags: Vec<String> = row
            .get("requires")
            .as_arr()
            .map(|items| items.iter().filter_map(|tag| tag.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if tags.is_empty() {
            return Err(format!("nav_overrides requirements[{index}] has no requires tags"));
        }
        requirements.push(Requirement { loc: row.get("loc").as_i64().unwrap_or(0) as u16, at, requires: tags, reason: reason.to_string() });
    }
    let mut one_way = Vec::new();
    let rows = parsed.get("one_way_doors").as_arr().cloned().unwrap_or_default();
    for (index, row) in rows.iter().enumerate() {
        let reason = row.get("reason").as_str().unwrap_or("");
        let side = row.get("passable_from").as_str().and_then(Side::parse);
        let Some(at) = parse_tile(row.get("at")) else {
            return Err(format!("nav_overrides one_way_doors[{index}] has no at tile"));
        };
        let (Some(side), false) = (side, reason.is_empty()) else {
            return Err(format!("nav_overrides one_way_doors[{index}] needs passable_from and reason"));
        };
        one_way.push(OneWay { loc: row.get("loc").as_i64().unwrap_or(0) as u16, at, passable_from: side, reason: reason.to_string() });
    }
    let mut customs = Vec::new();
    let rows = parsed.get("custom_edges").as_arr().cloned().unwrap_or_default();
    for (index, row) in rows.iter().enumerate() {
        let reason = row.get("reason").as_str().unwrap_or("");
        let name = row.get("name").as_str().unwrap_or("");
        let (Some(stand), Some(to)) = (parse_tile(row.get("stand")), parse_tile(row.get("to"))) else {
            return Err(format!("nav_overrides custom_edges[{index}] needs stand and to tiles"));
        };
        if name.is_empty() || reason.is_empty() {
            return Err(format!("nav_overrides custom_edges[{index}] needs name and reason"));
        }
        let tags: Vec<String> = row
            .get("requires")
            .as_arr()
            .map(|items| items.iter().filter_map(|tag| tag.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        customs.push(CustomEdge {
            name: name.to_string(),
            loc: row.get("loc").as_i64().unwrap_or(0) as u16,
            op: row.get("op").as_i64().unwrap_or(1) as u8,
            stand,
            to,
            requires: tags,
            reason: reason.to_string(),
        });
    }
    let mut blocked = Vec::new();
    let rows = parsed.get("blocked_locs").as_arr().cloned().unwrap_or_default();
    for (index, row) in rows.iter().enumerate() {
        let reason = row.get("reason").as_str().unwrap_or("");
        if reason.is_empty() {
            return Err(format!("nav_overrides blocked_locs[{index}] needs a reason"));
        }
        // No tile: the loctype table strips the whole loc. Only
        // tile-scoped entries land here; a malformed tile fails loud.
        if row.get("at").is_null() {
            continue;
        }
        let Some(at) = parse_tile(row.get("at")) else {
            return Err(format!("nav_overrides blocked_locs[{index}] has a malformed at tile"));
        };
        blocked.push(BlockedDoor {
            loc: row.get("loc").as_i64().unwrap_or(0) as u16,
            at,
            reason: reason.to_string(),
        });
    }
    Ok((requirements, one_way, customs, blocked))
}
