//! Supervised live checks: `check_doors`, `check_transitions`,
//! `check_routes` (plus `dump_bank_stands`). Sibling of `devtools`:
//! real teleport-then-walk loops over live state, tick-bounded.
//!
//! Wiring note: the crate root owns `pub mod devtools;` and
//! `pub mod devtools_check;`; this module reuses the `devtools` helpers.

use lifescape_gamedata::{all_locs, banks::all_banks};
use std::collections::HashMap;
use lifescape_sdk::{Bot, Error};

use crate::devtools::{await_landing, cheat_teleport, escape_json, live_loc_tiles, open_op, parse_filter, require_admin, save_report, sides_of};
use crate::{ScriptError, ScriptResult};

// -- check_doors ----------------------------------------------------------------
// One live instance per distinct door loc: teleport beside it, click the
// Open op, walk to the far side and check the arrival.
//
// shortcut: the wall-door scan (pack reader) and the router are not
// ported, so candidates come from the streamed live locs and the far
// side is the opposite orthogonal neighbour, not a routed tile.

fn live_doors(bot: &Bot, only: Option<&str>) -> Vec<(i32, i32, i32, u16, String)> {
    let mut seen: HashMap<u16, (i32, i32, i32, u16, String)> = HashMap::new();
    for ((x, z, level), value) in live_loc_tiles(bot) {
        let (loc_id, _, _) = value;
        if open_op(loc_id).is_none() {
            continue;
        }
        let name = (bot.tables.loc_name)(loc_id).unwrap_or_default();
        let lower = name.to_lowercase();
        if ["trapdoor", "ladder", "stair", "portal", "manhole"].iter().any(|key| lower.contains(key)) {
            continue;
        }
        if let Some(want) = only {
            if loc_id.to_string() != want {
                continue;
            }
        }
        seen.entry(loc_id).or_insert((x, z, level, loc_id, name));
    }
    let mut doors: Vec<_> = seen.into_values().collect();
    doors.sort();
    doors
}

fn open_live_door(bot: &mut Bot, x: i32, z: i32, level: i32, loc_id: u16) -> Result<bool, ScriptError> {
    let Some(op) = open_op(loc_id) else { return Ok(true) };
    if (bot.world.x, bot.world.z) == (x, z) && bot.world.level == level {
        let stepped = [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]
            .into_iter()
            .any(|(nx, nz)| bot.step_to(nx, nz, 60).unwrap_or(false));
        if !stepped {
            return Ok(false);
        }
    }
    let before = bot.world.loc_at(x, z, level).map(|loc| loc.0);
    bot.op_loc(op, x, z, loc_id)?;
    let deadline = bot.world.ticks + 60;
    while bot.world.ticks < deadline {
        bot.pump(2)?;
        if bot.world.loc_at(x, z, level).map(|loc| loc.0) != before {
            return Ok(true);
        }
    }
    Ok(bot.world.loc_at(x, z, level).map(|loc| loc.0) != before)
}

pub(crate) fn dev_check_doors(bot: &mut Bot, args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    let filter = parse_filter(args)?;
    bot.intend("live-testing doors");
    bot.ensure_all();
    let mut doors = live_doors(bot, filter.only.as_deref());
    doors = doors.into_iter().skip(filter.skip).take(if filter.limit > 0 { filter.limit } else { usize::MAX }).collect();
    let mut outcomes: Vec<(String, String)> = Vec::new();
    for (index, (x, z, level, loc_id, name)) in doors.iter().enumerate() {
        let label = format!("{loc_id}@{x},{z},{level}");
        let outcome = match test_door(bot, *x, *z, *level, *loc_id) {
            Ok(text) => text,
            Err(err) => format!("error: {err}"),
        };
        bot.log(&format!("{:4}/{:<30} {:<12} {outcome}", index + 1, label, name));
        if outcome != "ok" {
            let _ = cheat_teleport(bot, 0, 3222, 3218, 500);
        }
        outcomes.push((label, outcome));
    }
    let bad: Vec<_> = outcomes.iter().filter(|(_, outcome)| outcome != "ok").collect();
    bot.log(&format!("doors={} ok={} failed={}", outcomes.len(), outcomes.len() - bad.len(), bad.len()));
    for (label, outcome) in &bad {
        bot.log(&format!("  {label:<30} {outcome}"));
    }
    let mut body = String::from("{");
    for (i, (label, outcome)) in outcomes.iter().enumerate() {
        body.push_str(&format!("\"{}\":\"{}\"{}", escape_json(label), escape_json(outcome), if i + 1 < outcomes.len() { "," } else { "" }));
    }
    body.push('}');
    save_report("run/doors_test.json", &body)?;
    Ok(0)
}

fn test_door(bot: &mut Bot, x: i32, z: i32, level: i32, loc_id: u16) -> Result<String, ScriptError> {
    for ((ax, az), (bx, bz)) in [((x - 1, z), (x + 1, z)), ((x, z - 1), (x, z + 1))] {
        if !cheat_teleport(bot, level, ax, az, 500)? {
            continue;
        }
        if !open_live_door(bot, x, z, level, loc_id)? {
            continue;
        }
        match bot.walk_to(bx, bz, level, 1250) {
            Ok(()) => {}
            Err(err) => return Ok(format!("walk through: {err}")),
        }
        if (bot.world.x, bot.world.z) == (bx, bz) {
            return Ok("ok".to_string());
        }
        return Ok(format!("stopped at {:?}", bot.position()));
    }
    Ok("teleport failed".to_string())
}

// -- check_transitions ----------------------------------------------------------
// Table-driven live test: teleport beside each transition, click the
// climb op (pre-opening trapdoors), check the landing tile.
//
// shortcut: no resume file (the Python `--fresh` resume needs a results
// parser); no walkability ranking of sides (collision map not ported):
// sides are tried closest-first and the walk proves them.

#[derive(Debug, Clone)]
struct TransitionRow {
    name: String,
    kind: String,
    from: [i64; 3],
    to: [i64; 3],
    loc: u16,
    climb_loc: u16,
    climb_op: u8,
    open_to: Option<u16>,
    delta: Option<[i64; 3]>,
    footprint: Vec<[i64; 2]>,
    disabled: bool,
}

#[derive(Debug, Clone)]
enum MiniJson {
    Null,
    Int(i64),
    Str(String),
    Arr(Vec<MiniJson>),
}

struct MiniParser<'text> {
    bytes: &'text [u8],
    pos: usize,
}

impl<'text> MiniParser<'text> {
    fn parse(text: &'text str) -> Result<MiniJson, String> {
        let mut parser = MiniParser { bytes: text.as_bytes(), pos: 0 };
        let value = parser.value()?;
        parser.gap();
        if parser.pos != parser.bytes.len() {
            return Err("trailing bytes".to_string());
        }
        Ok(value)
    }

    fn gap(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<MiniJson, String> {
        self.gap();
        match self.bytes.get(self.pos) {
            Some(b'n') => self.word("null", MiniJson::Null),
            Some(b't') => self.word("true", MiniJson::Int(1)),
            Some(b'f') => self.word("false", MiniJson::Int(0)),
            Some(b'"') => Ok(MiniJson::Str(self.string()?)),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.gap();
                    if self.bytes.get(self.pos) == Some(&b']') {
                        self.pos += 1;
                        return Ok(MiniJson::Arr(items));
                    }
                    if !items.is_empty() {
                        if self.bytes.get(self.pos) != Some(&b',') {
                            return Err("expected , in array".to_string());
                        }
                        self.pos += 1;
                    }
                    items.push(self.value()?);
                }
            }
            Some(b'{') => {
                // Objects collapse to their values in key order; the caller
                // reads rows positionally, so keys are matched by name here.
                self.pos += 1;
                let mut fields = Vec::new();
                loop {
                    self.gap();
                    if self.bytes.get(self.pos) == Some(&b'}') {
                        self.pos += 1;
                        return Ok(MiniJson::Arr(fields));
                    }
                    if !fields.is_empty() {
                        if self.bytes.get(self.pos) != Some(&b',') {
                            return Err("expected , in object".to_string());
                        }
                        self.pos += 1;
                        self.gap();
                    }
                    if self.bytes.get(self.pos) != Some(&b'"') {
                        return Err("expected string key".to_string());
                    }
                    let key = self.string()?;
                    self.gap();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("expected : after key".to_string());
                    }
                    self.pos += 1;
                    let value = self.value()?;
                    fields.push(MiniJson::Arr(vec![MiniJson::Str(key), value]));
                }
            }
            Some(byte) if *byte == b'-' || byte.is_ascii_digit() => Ok(MiniJson::Int(self.number()?)),
            _ => Err("unexpected value".to_string()),
        }
    }

    fn word(&mut self, word: &str, value: MiniJson) -> Result<MiniJson, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err("bad literal".to_string())
        }
    }

    fn number(&mut self) -> Result<i64, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "bad number".to_string())?
            .parse()
            .map_err(|_| "bad number".to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let byte = *self.bytes.get(self.pos).ok_or("unterminated string".to_string())?;
            self.pos += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = *self.bytes.get(self.pos).ok_or("bad escape".to_string())?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        _ => out.push('?'),
                    }
                }
                other => out.push(other as char),
            }
        }
    }
}

fn field<'a>(fields: &'a [MiniJson], key: &str) -> Option<&'a MiniJson> {
    fields.iter().find_map(|entry| match entry {
        MiniJson::Arr(pair) if pair.len() == 2 => match &pair[0] {
            MiniJson::Str(name) if name == key => Some(&pair[1]),
            _ => None,
        },
        _ => None,
    })
}

fn int_triplet(value: &MiniJson) -> Option<[i64; 3]> {
    match value {
        MiniJson::Arr(items) if items.len() == 3 => match (&items[0], &items[1], &items[2]) {
            (MiniJson::Int(a), MiniJson::Int(b), MiniJson::Int(c)) => Some([*a, *b, *c]),
            _ => None,
        },
        _ => None,
    }
}

fn load_transitions() -> Result<Vec<TransitionRow>, ScriptError> {
    let mut text = None;
    for path in ["gamedata/data/transitions.json"] {
        if let Ok(body) = std::fs::read_to_string(path) {
            text = Some(body);
            break;
        }
    }
    let text = text.ok_or_else(|| {
        ScriptError::Usage("transitions.json not found (run orchestrator dump-transitions gamedata/data/transitions.json first)".to_string())
    })?;
    let parsed = MiniParser::parse(&text).map_err(|err| ScriptError::Usage(format!("bad transitions.json: {err}")))?;
    let MiniJson::Arr(rows) = parsed else { return Err(ScriptError::Usage("transitions.json is not an array".to_string())) };
    let mut out = Vec::new();
    for row in rows {
        let MiniJson::Arr(fields) = row else { continue };
        let triplet = |key: &str| field(&fields, key).and_then(int_triplet);
        let (Some(from), Some(to)) = (triplet("from"), triplet("to")) else { continue };
        let int = |key: &str| match field(&fields, key) {
            Some(MiniJson::Int(n)) => *n,
            _ => 0,
        };
        let footprint = match field(&fields, "footprint") {
            Some(MiniJson::Arr(tiles)) => tiles
                .iter()
                .filter_map(|tile| match tile {
                    MiniJson::Arr(pair) if pair.len() == 2 => match (&pair[0], &pair[1]) {
                        (MiniJson::Int(a), MiniJson::Int(b)) => Some([*a, *b]),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => vec![[from[1], from[2]]],
        };
        out.push(TransitionRow {
            name: match field(&fields, "name") {
                Some(MiniJson::Str(name)) => name.clone(),
                _ => format!("{}-{}-{}", from[0], from[1], from[2]),
            },
            kind: match field(&fields, "kind") {
                Some(MiniJson::Str(kind)) => kind.clone(),
                _ => "other".to_string(),
            },
            from,
            to,
            loc: int("loc") as u16,
            climb_loc: int("climb_loc") as u16,
            climb_op: int("climb_op").max(1) as u8,
            open_to: match field(&fields, "open_to") {
                Some(MiniJson::Int(id)) => Some(*id as u16),
                _ => None,
            },
            delta: field(&fields, "delta").and_then(int_triplet),
            footprint,
            disabled: !matches!(field(&fields, "disabled"), None | Some(MiniJson::Null)),
        });
    }
    Ok(out)
}

fn test_transition(bot: &mut Bot, row: &TransitionRow) -> Result<String, ScriptError> {
    let (level, x, z) = (row.from[0] as i32, row.from[1] as i32, row.from[2] as i32);
    let sides = sides_of(&row.footprint.iter().map(|tile| [tile[0], tile[1]]).collect::<Vec<_>>(), bot.world.x, bot.world.z);
    if sides.is_empty() {
        return Ok("no walkable side".to_string());
    }
    let deadline = bot.world.ticks + 600;
    for (side_x, side_z) in sides {
        if bot.world.ticks > deadline {
            break;
        }
        if !cheat_teleport(bot, level, side_x, side_z, 250)? {
            continue;
        }
        let expected = match row.delta {
            Some([dlevel, dx, dz]) => (level + dlevel as i32, side_x + dx as i32, side_z + dz as i32),
            None => (row.to[0] as i32, row.to[1] as i32, row.to[2] as i32),
        };
        if let Some(opened) = row.open_to {
            if bot.world.loc_at(x, z, level).map(|loc| loc.0) != Some(opened) {
                bot.op_loc(1, x, z, if row.loc != 0 { row.loc } else { row.climb_loc })?;
                let opened_by = bot.world.ticks + 20;
                while bot.world.ticks < opened_by {
                    bot.pump(2)?;
                    if bot.world.loc_at(x, z, level).map(|loc| loc.0) == Some(opened) {
                        break;
                    }
                }
            }
        }
        bot.op_loc(row.climb_op, x, z, row.climb_loc)?;
        if await_landing(bot, expected, 150)? {
            return Ok("ok".to_string());
        }
    }
    Ok(format!("ride: ended at {:?}", bot.position()))
}

pub(crate) fn dev_check_transitions(bot: &mut Bot, args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    let filter = parse_filter(args)?;
    bot.intend("live-testing transitions");
    let mut rows = load_transitions()?;
    rows.retain(|row| !row.disabled);
    if let Some(only) = &filter.only {
        rows.retain(|row| row.name.contains(only));
    }
    if let Some(kind) = &filter.kind {
        rows.retain(|row| row.kind == *kind);
    }
    rows = rows.into_iter().skip(filter.skip).take(if filter.limit > 0 { filter.limit } else { usize::MAX }).collect();
    bot.log(&format!("testing {} transitions", rows.len()));
    let mut outcomes: Vec<(String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let outcome = match test_transition(bot, row) {
            Ok(text) => text,
            Err(err) => format!("error: {err}"),
        };
        bot.log(&format!("{:4}/{} {:<34} {outcome}", index + 1, rows.len(), row.name));
        if outcome != "ok" {
            let _ = cheat_teleport(bot, 0, 3222, 3218, 500);
        }
        outcomes.push((format!("{},{},{}>{},{},{}@{}.{}", row.from[0], row.from[1], row.from[2], row.to[0], row.to[1], row.to[2], row.climb_loc, row.climb_op), outcome));
    }
    let bad = outcomes.iter().filter(|(_, outcome)| outcome != "ok").count();
    bot.log(&format!("tested={} ok={} failed={}", outcomes.len(), outcomes.len() - bad, bad));
    let mut body = String::from("{");
    for (i, (key, outcome)) in outcomes.iter().enumerate() {
        body.push_str(&format!("\"{}\":\"{}\"{}", escape_json(key), escape_json(outcome), if i + 1 < outcomes.len() { "," } else { "" }));
    }
    body.push('}');
    save_report("run/transitions_test.json", &body)?;
    Ok(0)
}

// -- check_routes ---------------------------------------------------------------
// City backbones plus bank->tree/rock/booth legs, teleported then walked
// for real. Blocked destinations (tree, rock, booth) go through
// `approach`, open ground through `walk_to`.
//
// shortcut: the F2P planner pre-check is not ported (no pack reader), so
// every leg is attempted live instead of pre-validated.

const BACKBONE: &[(&str, (i32, i32, i32), (i32, i32, i32))] = &[
    ("lumbridge-varrock", (0, 3222, 3218), (0, 3205, 3429)),
    ("varrock-lumbridge", (0, 3205, 3429), (0, 3222, 3218)),
    ("varrock-falador", (0, 3205, 3429), (0, 2965, 3378)),
    ("falador-port-sarim", (0, 2965, 3378), (0, 3030, 3220)),
    ("port-sarim-draynor", (0, 3030, 3220), (0, 3092, 3242)),
];

const TREE_NAMES: &[&str] = &["tree", "oaktree", "willowtree"];
const ROCK_NAMES: &[&str] = &["clayrock1", "clayrock2", "copperrock1", "copperrock2", "tinrock1", "tinrock2", "ironrock1", "ironrock2"];

fn loc_spawns_near(want: &str, near_x: i64, near_z: i64, level: i64, radius: i64) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in all_locs() {
        if entry.name.as_deref().unwrap_or("").to_lowercase() != want {
            continue;
        }
        for spot in &entry.spawns {
            if spot.level == level && (spot.x - near_x).abs() + (spot.z - near_z).abs() <= radius {
                found.push((spot.x as i32, spot.z as i32));
            }
        }
    }
    found.sort_by_key(|spot| (spot.0 as i64 - near_x).abs() + (spot.1 as i64 - near_z).abs());
    found
}

struct RouteLeg {
    name: String,
    start: (i32, i32, i32),
    dest: (i32, i32, i32),
    expect: &'static str,
}

fn build_route_legs() -> Vec<RouteLeg> {
    let mut legs: Vec<RouteLeg> = BACKBONE
        .iter()
        .map(|(name, start, dest)| RouteLeg { name: (*name).to_string(), start: *start, dest: *dest, expect: "reach" })
        .collect();
    let mut banks: Vec<_> = all_banks().into_iter().filter(|bank| !bank.members && bank.level == 0).collect();
    banks.sort_by_key(|bank| (bank.x, bank.z));
    let (mut trees, mut rocks, mut booths) = (0, 0, 0);
    for bank in banks {
        if trees < 2 {
            for tree in TREE_NAMES {
                if let Some(spot) = loc_spawns_near(tree, bank.x, bank.z, 0, 60).into_iter().next() {
                    legs.push(RouteLeg {
                        name: format!("bank-tree-{}-{}", bank.x, bank.z),
                        start: (0, bank.x as i32, bank.z as i32),
                        dest: (0, spot.0, spot.1),
                        expect: "reach",
                    });
                    trees += 1;
                    break;
                }
            }
        }
        if rocks < 2 {
            for rock in ROCK_NAMES {
                if let Some(spot) = loc_spawns_near(rock, bank.x, bank.z, 0, 60).into_iter().next() {
                    legs.push(RouteLeg {
                        name: format!("bank-rock-{}-{}", bank.x, bank.z),
                        start: (0, bank.x as i32, bank.z as i32),
                        dest: (0, spot.0, spot.1),
                        expect: "reach",
                    });
                    rocks += 1;
                    break;
                }
            }
        }
        if booths < 2 {
            legs.push(RouteLeg {
                name: format!("bank-booth-{}-{}", bank.x, bank.z),
                start: (0, bank.x as i32 + 6, bank.z as i32),
                dest: (0, bank.x as i32, bank.z as i32),
                expect: "reach",
            });
            booths += 1;
        }
        if trees >= 2 && rocks >= 2 && booths >= 2 {
            break;
        }
    }
    legs
}

fn find_toll_gate(bot: &Bot) -> Option<(i32, i32, i32)> {
    for ((x, z, level), (loc_id, _, _)) in &bot.world.static_locs {
        if *level != 0 {
            continue;
        }
        if (bot.tables.loc_name)(*loc_id).unwrap_or_default().to_lowercase().contains("toll") {
            return Some((0, *x, *z));
        }
    }
    None
}

pub(crate) fn dev_check_routes(bot: &mut Bot, args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    let filter = parse_filter(args)?;
    bot.intend("live-testing routes");
    bot.ensure_all();
    let mut legs = build_route_legs();
    if let Some(gate) = find_toll_gate(bot) {
        let (level, gx, gz) = gate;
        legs.push(RouteLeg {
            name: "toll-gate".to_string(),
            start: (level, gx - 8, gz),
            dest: (level, gx + 8, gz),
            expect: "diagnose",
        });
    } else {
        bot.log("no toll gate in live locs, skipping");
    }
    if let Some(only) = &filter.only {
        legs.retain(|leg| leg.name.contains(only));
    }
    legs = legs.into_iter().skip(filter.skip).take(if filter.limit > 0 { filter.limit } else { usize::MAX }).collect();
    bot.log(&format!("testing {} legs", legs.len()));
    let mut outcomes: Vec<(String, String)> = Vec::new();
    for (index, leg) in legs.iter().enumerate() {
        let outcome = match test_leg(bot, leg) {
            Ok(text) => text,
            Err(err) => format!("error: {err}"),
        };
        bot.log(&format!("{:4}/{} {:<22} {outcome}", index + 1, legs.len(), leg.name));
        if !outcome.starts_with("ok") {
            let _ = cheat_teleport(bot, 0, 3222, 3218, 500);
        }
        outcomes.push((leg.name.clone(), outcome));
    }
    let (mut ok, mut skipped) = (0, 0);
    for (_, outcome) in &outcomes {
        if outcome.starts_with("ok") {
            ok += 1;
        } else if outcome.starts_with("skipped") {
            skipped += 1;
        }
    }
    bot.log(&format!("tested={} ok={} failed={} skipped={}", outcomes.len(), ok, outcomes.len() - ok - skipped, skipped));
    let mut body = String::from("{");
    for (i, (name, outcome)) in outcomes.iter().enumerate() {
        body.push_str(&format!("\"{}\":\"{}\"{}", escape_json(name), escape_json(outcome), if i + 1 < outcomes.len() { "," } else { "" }));
    }
    body.push('}');
    save_report("run/routes_test.json", &body)?;
    Ok(0)
}

fn test_leg(bot: &mut Bot, leg: &RouteLeg) -> Result<String, ScriptError> {
    if !cheat_teleport(bot, leg.start.0, leg.start.1, leg.start.2, 250)? {
        return Ok("teleport failed".to_string());
    }
    let blocked = bot.world.loc_at(leg.dest.1, leg.dest.2, leg.dest.0).is_some();
    let spent = bot.world.ticks;
    let walk = if blocked {
        bot.approach(leg.dest.1, leg.dest.2, leg.dest.0, 2, 3000)
    } else {
        bot.walk_to(leg.dest.1, leg.dest.2, leg.dest.0, 3000)
    };
    let spent = bot.world.ticks - spent;
    match walk {
        Ok(()) => {
            if leg.expect == "diagnose" {
                Ok("ok (walked through, had coins?)".to_string())
            } else {
                Ok(format!("ok in {spent} ticks"))
            }
        }
        Err(Error::Unreachable(detail)) if leg.expect == "diagnose" && detail.to_lowercase().contains("toll") => {
            Ok("ok (diagnosed toll)".to_string())
        }
        Err(err) => Ok(format!("unreachable: {err}")),
    }
}

// -- dump_bank_stands ------------------------------------------------------------
// Booth clusters from gamedata, the part of the dump that needs no
// collision map. Stand selection (BFS flood + reach check) stays Python:
// the flood, door and reach modules are not ported.
//
// shortcut: stands are NOT computed; the JSON carries booths + centroid
// so a staff run can verify stands live instead.

pub(crate) fn dev_dump_bank_stands(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    bot.intend("dumping bank booth clusters");
    bot.ensure_all();
    let mut banks: Vec<_> = all_banks().into_iter().filter(|bank| !bank.members && bank.level == 0).collect();
    banks.sort_by_key(|bank| (bank.x, bank.z));
    let mut clusters: Vec<Vec<(i64, i64)>> = Vec::new();
    for bank in &banks {
        let mut placed = false;
        for cluster in clusters.iter_mut() {
            if cluster.iter().any(|(x, z)| (bank.x - x).abs() + (bank.z - z).abs() <= 15) {
                cluster.push((bank.x, bank.z));
                placed = true;
                break;
            }
        }
        if !placed {
            clusters.push(vec![(bank.x, bank.z)]);
        }
    }
    let mut body = String::from("[");
    for (i, cluster) in clusters.iter().enumerate() {
        let cx = cluster.iter().map(|tile| tile.0).sum::<i64>() / cluster.len() as i64;
        let cz = cluster.iter().map(|tile| tile.1).sum::<i64>() / cluster.len() as i64;
        let booths = cluster.iter().map(|(x, z)| format!("[{x},{z}]")).collect::<Vec<_>>().join(",");
        body.push_str(&format!(
            "{{\"bank\":[{0},{1}],\"centroid\":[{cx},{cz}],\"booths\":[{booths}]}}",
            cluster[0].0, cluster[0].1
        ));
        if i + 1 < clusters.len() {
            body.push(',');
        }
    }
    body.push(']');
    save_report("run/bank_stands.json", &body)?;
    bot.log(&format!("wrote run/bank_stands.json ({} clusters, stands not computed: reach module not ported)", clusters.len()));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::MiniParser;

    #[test]
    fn mini_json_reads_rows() {
        let parsed = MiniParser::parse(r#"[{"name":"rope","from":[0,1,2],"to":[0,3,4],"disabled":null}]"#).expect("parses");
        match parsed {
            super::MiniJson::Arr(rows) => assert_eq!(rows.len(), 1),
            _ => panic!("expected array"),
        }
    }
}
