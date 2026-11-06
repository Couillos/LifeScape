//! Build the stair/ladder transition table from the server scripts.
//!
//! One row per click that moves the player to another tile: stairs,
//! ladders, trapdoors, manholes, ropes and portals. Source coordinates
//! come from the `oploc` handlers in lostcity/content, loc ids from the
//! map placements and the cache loc types. Only tiles on free-to-play
//! ground (level ignored, like the engine's own check) are kept.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use lifescape_sdk::buffer::Reader;
use lifescape_sdk::collision::{CollisionMap, change_loc_collision, zip_index, zip_slice};

use crate::json::{self, Value};
use crate::paths;

/// One static loc placement on free-to-play ground.
struct Spot {
    tile_x: i32,
    tile_z: i32,
    level: i32,
    loc_id: i32,
    shape: u8,
    angle: u8,
}

/// A parsed transition row, ready to serialize.
struct Row {
    name: String,
    kind: String,
    from_tile: (i32, i32, i32),
    to_tile: (i32, i32, i32),
    loc: i32,
    climb_loc: i32,
    climb_op: u8,
    open_to: Option<i32>,
    delta: Option<(i32, i32, i32)>,
    footprint: Vec<(i32, i32)>,
    disabled: Option<&'static str>,
}

/// Engine checkout next to the project root (same layout `run_direct` uses).
fn engine_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    paths::root().join("../lostcity/engine")
}

fn content_dir() -> PathBuf {
    engine_dir().join("../content")
}

fn gametypes_path() -> PathBuf {
    paths::root().join("lifescape").join("data").join("gametypes.json")
}

/// Zone keys (x_zone, z_zone) of the free-to-play map, level ignored.
fn f2p_zones(content: &Path) -> Result<HashSet<(i32, i32)>, String> {
    let path = content.join("maps").join("free2play.csv");
    let text = std::fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let mut zones = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('_').collect();
        if parts.len() != 5 {
            continue;
        }
        let mut nums = [0i32; 5];
        let mut ok = true;
        for (slot, part) in parts.iter().enumerate() {
            match part.parse::<i32>() {
                Ok(num) => nums[slot] = num,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        zones.insert((nums[1] * 8 + (nums[3] >> 3), nums[2] * 8 + (nums[4] >> 3)));
    }
    Ok(zones)
}

/// Raw bytes of every loc placement packet, sorted by entry name.
fn loc_packets(pack_dir: &Path) -> Result<Vec<(i32, i32, Vec<u8>)>, String> {
    let zipped = pack_dir.join(".cache").join("maps-server.zip");
    if zipped.is_file() {
        let bytes = std::fs::read(&zipped).map_err(|err| format!("read {}: {err}", zipped.display()))?;
        let index = zip_index(&bytes)?;
        let mut names: Vec<&String> = index.keys().filter(|name| name.starts_with('l')).collect();
        names.sort();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let coords = name[1..].split('_').collect::<Vec<_>>();
            if coords.len() != 2 {
                continue;
            }
            let (Some(mx), Some(mz)) = (coords[0].parse::<i32>().ok(), coords[1].parse::<i32>().ok()) else {
                continue;
            };
            out.push((mx << 6, mz << 6, zip_slice(&bytes, index[name]).to_vec()));
        }
        return Ok(out);
    }
    let maps_dir = pack_dir.join("server").join("maps");
    let mut names = Vec::new();
    let entries = std::fs::read_dir(&maps_dir).map_err(|err| format!("read {}: {err}", maps_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {}: {err}", maps_dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('l') {
            names.push(name);
        }
    }
    names.sort();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let coords = name[1..].split('_').collect::<Vec<_>>();
        if coords.len() != 2 {
            continue;
        }
        let (Some(mx), Some(mz)) = (coords[0].parse::<i32>().ok(), coords[1].parse::<i32>().ok()) else {
            continue;
        };
        let bytes = std::fs::read(maps_dir.join(&name)).map_err(|err| format!("read {name}: {err}"))?;
        out.push((mx << 6, mz << 6, bytes));
    }
    Ok(out)
}

/// Every static loc on free-to-play ground, duplicates included.
fn placements(pack_dir: &Path, zones: &HashSet<(i32, i32)>) -> Result<Vec<Spot>, String> {
    let mut spots = Vec::new();
    for (square_x, square_z, packet) in loc_packets(pack_dir)? {
        let mut reader = Reader::new(&packet);
        let mut loc_id: i32 = -1;
        let mut offset = reader.smarts().map_err(|err| format!("loc packet: {err}"))?;
        while offset != 0 {
            loc_id += i32::from(offset);
            let mut coord: i32 = 0;
            let mut coord_offset = reader.smarts().map_err(|err| format!("loc packet: {err}"))?;
            while coord_offset != 0 {
                coord += i32::from(coord_offset) - 1;
                let tile_z = coord & 0x3F;
                let tile_x = (coord >> 6) & 0x3F;
                let level = (coord >> 12) & 0x3;
                let info = reader.u8().map_err(|err| format!("loc packet: {err}"))?;
                coord_offset = reader.smarts().map_err(|err| format!("loc packet: {err}"))?;
                let absolute_x = square_x + tile_x;
                let absolute_z = square_z + tile_z;
                if zones.contains(&(absolute_x >> 3, absolute_z >> 3)) {
                    spots.push(Spot {
                        tile_x: absolute_x,
                        tile_z: absolute_z,
                        level,
                        loc_id,
                        shape: info >> 2,
                        angle: info & 0x3,
                    });
                }
            }
            offset = reader.smarts().map_err(|err| format!("loc packet: {err}"))?;
        }
    }
    Ok(spots)
}

/// `[oplocN,name]` blocks of one script file.
fn script_blocks(text: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut header: Option<String> = None;
    let mut body: Vec<&str> = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') && stripped.len() >= 2 {
            if let Some(open) = header.take() {
                blocks.push((open, body.join("\n")));
                body.clear();
            }
            header = Some(stripped[1..stripped.len() - 1].to_string());
        } else if header.is_some() {
            body.push(line);
        }
    }
    if let Some(open) = header.take() {
        blocks.push((open, body.join("\n")));
    }
    blocks
}

/// Split `oploc3,laddertop` into op and handler name.
fn parse_header(header: &str) -> Option<(u8, String)> {
    let rest = header.strip_prefix("oploc")?;
    let op = rest.as_bytes().first()?;
    if !op.is_ascii_digit() || *op < b'1' || *op > b'5' {
        return None;
    }
    let name = rest[1..].strip_prefix(',')?;
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_') {
        return None;
    }
    Some((op - b'0', name.to_string()))
}

fn collect_rs2(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = Vec::new();
    let listing = std::fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))?;
    for entry in listing {
        let entry = entry.map_err(|err| format!("read {}: {err}", dir.display()))?;
        entries.push(entry.path());
    }
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rs2(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs2") {
            out.push(path);
        }
    }
    Ok(())
}

/// `{(loc name, op): [body]}` for every `oploc` handler.
fn handlers(content: &Path) -> Result<BTreeMap<(String, u8), Vec<String>>, String> {
    let mut found: BTreeMap<(String, u8), Vec<String>> = BTreeMap::new();
    let mut files = Vec::new();
    collect_rs2(&content.join("scripts"), &mut files)?;
    for path in files {
        let text = std::fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        for (header, body) in script_blocks(&text) {
            if let Some((op, name)) = parse_header(&header) {
                found.entry((name, op)).or_default().push(body);
            }
        }
    }
    Ok(found)
}

fn packed_key(level: i32, tile_x: i32, tile_z: i32) -> String {
    format!("{}_{}_{}_{}_{}", level, tile_x >> 6, tile_z >> 6, tile_x & 0x3F, tile_z & 0x3F)
}

/// Rest of a `case <key>:` line, with multi-line chunks gathered like the
/// source (a case whose own line is empty or a comment owns the lines up
/// to the next case or closing brace).
fn match_case_line(stripped: &str) -> Option<(String, String)> {
    let rest = stripped.strip_prefix("case")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let end = rest.find(':')?;
    let (key, after) = (rest[..end].trim_end().to_string(), rest[end + 1..].trim_start().to_string());
    if key != "default" && (key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_digit() || byte == b'_')) {
        return None;
    }
    Some((key, after))
}

/// Body of the `switch_coord` case for this tile, or the default case.
fn case_body(body: &str, from_tile: (i32, i32, i32)) -> Option<String> {
    let key = packed_key(from_tile.0, from_tile.1, from_tile.2);
    let mut fallback: Option<String> = None;
    let lines: Vec<&str> = body.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let Some((case_key, chunk)) = match_case_line(lines[index].trim()) else {
            index += 1;
            continue;
        };
        index += 1;
        let mut chunk = chunk;
        if chunk.trim().is_empty() || chunk.trim().starts_with("//") {
            let mut extra = Vec::new();
            while index < lines.len() {
                let ahead = lines[index].trim();
                if match_case_line(ahead).is_some() || ahead == "}" {
                    break;
                }
                extra.push(lines[index]);
                index += 1;
            }
            if !extra.is_empty() {
                chunk.push('\n');
                chunk.push_str(&extra.join("\n"));
            } else if !chunk.contains('\n') {
                chunk.push('\n');
            }
        }
        if case_key == key {
            return Some(chunk);
        }
        if case_key == "default" && fallback.is_none() {
            fallback = Some(chunk);
        }
    }
    fallback.or_else(|| switch_tail(body))
}

/// Code after the first `switch_coord` block (its generic path).
fn switch_tail(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("switch_coord") {
            continue;
        }
        let mut depth = 0;
        let mut started = false;
        for (rest, tail) in lines.iter().enumerate().skip(index) {
            for ch in tail.chars() {
                if ch == '{' {
                    depth += 1;
                    started = true;
                } else if ch == '}' {
                    depth -= 1;
                    if started && depth == 0 {
                        return Some(lines[rest + 1..].join("\n"));
                    }
                }
            }
        }
        return None;
    }
    None
}

/// Body of the `loc_angle` case for this placement.
fn angle_body(body: &str, angle: u8) -> Option<String> {
    for line in body.lines() {
        let Some((case_key, chunk)) = match_case_line(line.trim()) else {
            continue;
        };
        if case_key == angle.to_string() || case_key == "default" {
            return Some(chunk);
        }
    }
    None
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// First `level_mx_mz_lx_lz` absolute target on the line, if any.
fn find_absolute(line: &str) -> Option<(i32, i32, i32, i32, i32)> {
    let bytes = line.as_bytes();
    let mut run_start: Option<usize> = None;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (at, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_digit() {
            if run_start.is_none() {
                run_start = Some(at);
            }
        } else if let Some(start) = run_start.take() {
            runs.push((start, at));
        }
    }
    if let Some(start) = run_start.take() {
        runs.push((start, bytes.len()));
    }
    for window in runs.windows(5) {
        let mut ok = true;
        for pair in window.windows(2) {
            if pair[1].0 != pair[0].1 + 1 || bytes[pair[0].1] != b'_' {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        if window[0].0 > 0 && is_word_char(bytes[window[0].0 - 1]) {
            continue;
        }
        if window[4].1 < bytes.len() && is_word_char(bytes[window[4].1]) {
            continue;
        }
        let mut nums = [0i32; 5];
        for (slot, (start, end)) in window.iter().enumerate() {
            match line[*start..*end].parse::<i32>() {
                Ok(num) => nums[slot] = num,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some((nums[0], nums[1], nums[2], nums[3], nums[4]));
        }
    }
    None
}

fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

fn parse_int(bytes: &[u8], mut pos: usize) -> Option<(i32, usize)> {
    pos = skip_ws(bytes, pos);
    let start = pos;
    if pos < bytes.len() && bytes[pos] == b'-' {
        pos += 1;
    }
    let digits = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }
    if pos == digits {
        return None;
    }
    std::str::from_utf8(&bytes[start..pos]).ok()?.parse::<i32>().ok().map(|num| (num, pos))
}

/// `(movecoord args)` after the opening paren: player-relative or loc-relative.
fn parse_movecoord(bytes: &[u8], pos: usize) -> Option<(bool, [i32; 3], usize)> {
    let mut pos = skip_ws(bytes, pos);
    let player = if bytes.get(pos..pos + 9) == Some(b"loc_coord") {
        pos += 9;
        false
    } else if bytes.get(pos..pos + 5) == Some(b"coord") {
        pos += 5;
        true
    } else {
        return None;
    };
    pos = skip_ws(bytes, pos);
    if bytes.get(pos..pos + 2) == Some(b"()") {
        pos += 2;
    }
    pos = skip_ws(bytes, pos);
    if bytes.get(pos) != Some(&b',') {
        return None;
    }
    pos += 1;
    let (first, next) = parse_int(bytes, pos)?;
    pos = skip_ws(bytes, next);
    if bytes.get(pos) != Some(&b',') {
        return None;
    }
    let (second, next) = parse_int(bytes, pos + 1)?;
    pos = skip_ws(bytes, next);
    if bytes.get(pos) != Some(&b',') {
        return None;
    }
    let (third, next) = parse_int(bytes, pos + 1)?;
    pos = skip_ws(bytes, next);
    if bytes.get(pos) != Some(&b')') {
        return None;
    }
    Some((player, [first, second, third], pos + 1))
}

fn has_teleport(line: &str) -> bool {
    for keyword in ["p_telejump", "p_teleport", "climb_ladder", "tele_jump", "teleport"] {
        let mut from = 0;
        while let Some(at) = line[from..].find(keyword) {
            let after = from + at + keyword.len();
            if line.as_bytes().get(after) == Some(&b'(') {
                return true;
            }
            from = after;
        }
    }
    false
}

/// `movecoord` args are (x offset, level offset, z offset).
fn moved(offsets: [i32; 3], from_tile: (i32, i32, i32)) -> ((i32, i32, i32), (i32, i32, i32)) {
    let (dx, dlevel, dz) = (offsets[0], offsets[1], offsets[2]);
    ((from_tile.0 + dlevel, from_tile.1 + dx, from_tile.2 + dz), (dlevel, dx, dz))
}

/// Destination of the first teleport in the text, with the player-relative
/// delta when the script moves whoever clicked from where they stand.
fn destination(text: &str, from_tile: (i32, i32, i32)) -> (Option<(i32, i32, i32)>, Option<(i32, i32, i32)>) {
    for line in text.lines() {
        if !has_teleport(line) {
            continue;
        }
        if let Some((level, mx, mz, lx, lz)) = find_absolute(line) {
            return (Some((level, mx * 64 + lx, mz * 64 + lz)), None);
        }
        let bytes = line.as_bytes();
        let mut from = 0;
        let mut args: Option<(bool, [i32; 3])> = None;
        while let Some(at) = line[from..].find("movecoord(") {
            let open = from + at + "movecoord(".len();
            if let Some((player, nums, _)) = parse_movecoord(bytes, open) {
                args = Some((player, nums));
                break;
            }
            from = open;
        }
        if let Some((player, nums)) = args {
            if player {
                let (target, delta) = moved(nums, from_tile);
                return (Some(target), Some(delta));
            }
            let (target, _) = moved(nums, from_tile);
            return (Some(target), None);
        }
    }
    (None, None)
}

fn joined_body(bodies: &[String]) -> String {
    bodies.join("\n")
}

/// Destination of the handler, following the coord/angle switch first.
fn resolved(
    name: &str,
    op: u8,
    handled: &BTreeMap<(String, u8), Vec<String>>,
    from_tile: (i32, i32, i32),
    angle: u8,
) -> Option<(Option<(i32, i32, i32)>, Option<(i32, i32, i32)>)> {
    let bodies = handled.get(&(name.to_string(), op))?;
    let body = joined_body(bodies);
    let active = if body.contains("switch_coord") {
        case_body(&body, from_tile)?
    } else if body.contains("switch_int") && body.contains("loc_angle") {
        angle_body(&body, angle)?
    } else {
        body
    };
    Some(destination(&active, from_tile))
}

/// `(opened name, op, destination)` when the click opens a trapdoor first.
fn opened_to(
    name: &str,
    op: u8,
    handled: &BTreeMap<(String, u8), Vec<String>>,
    from_tile: (i32, i32, i32),
    angle: u8,
) -> Option<(String, u8, (Option<(i32, i32, i32)>, Option<(i32, i32, i32)>))> {
    let bodies = handled.get(&(name.to_string(), op))?;
    let body = joined_body(bodies);
    let active = if body.contains("switch_coord") {
        case_body(&body, from_tile)?
    } else if body.contains("switch_int") && body.contains("loc_angle") {
        angle_body(&body, angle)?
    } else {
        body
    };
    let opened = find_opened(&active)?;
    let mut ops: Vec<u8> = handled.keys().filter(|(candidate, _)| candidate == &opened).map(|(_, op)| *op).collect();
    ops.sort_unstable();
    for next_op in ops {
        let found = resolved(&opened, next_op, handled, from_tile, angle)?;
        if found.0.is_some() {
            return Some((opened, next_op, found));
        }
    }
    None
}

fn find_opened(body: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = body[from..].find("loc_change(") {
        let mut pos = from + at + "loc_change(".len();
        let bytes = body.as_bytes();
        pos = skip_ws(bytes, pos);
        let start = pos;
        while pos < bytes.len() && (bytes[pos].is_ascii_lowercase() || bytes[pos].is_ascii_digit() || bytes[pos] == b'_') {
            pos += 1;
        }
        if pos > start {
            return Some(body[start..pos].to_string());
        }
        from = pos + 1;
    }
    None
}

/// Every tile the loc covers: a staircase is entered from any edge.
fn footprint_tiles(
    defs: &[Option<lifescape_sdk::collision::LocDef>],
    level: i32,
    tile_x: i32,
    tile_z: i32,
    loc_id: i32,
    shape: u8,
    angle: u8,
) -> Vec<(i32, i32)> {
    let entry = if loc_id >= 0 && (loc_id as usize) < defs.len() {
        defs[loc_id as usize]
    } else {
        None
    };
    let Some(def) = entry else {
        return vec![(tile_x, tile_z)];
    };
    if shape <= 3 {
        return vec![(tile_x, tile_z)];
    }
    let mut probe = CollisionMap::new();
    change_loc_collision(&mut probe, shape, angle, def.blockrange, def.length, def.width, def.active, tile_x, tile_z, level, true);
    // Unsigned 8x8 bases, like the packed format walk: dungeon tiles sit
    // past z 8192, where a signed read would wrap negative.
    let mut tiles: Vec<(i32, i32)> = Vec::new();
    for (zone_key, zone) in probe.raw_zones() {
        let base_x = ((zone_key & 0x7FF) << 3) as i32;
        let base_z = (((zone_key >> 11) & 0x7FF) << 3) as i32;
        for (slot, flags) in zone.iter().enumerate() {
            if *flags != 0 {
                let slot = slot as i32;
                tiles.push((base_x + (slot & 0x7), base_z + ((slot >> 3) & 0x7)));
            }
        }
    }
    tiles.sort_unstable();
    if tiles.is_empty() {
        tiles.push((tile_x, tile_z));
    }
    tiles
}

fn kind_of(name: &str) -> &'static str {
    if name.contains("stair") {
        "stair"
    } else if name.contains("ladder") || name.contains("rope") || name.contains("climb") {
        "ladder"
    } else if name.contains("trapdoor") || name.contains("manhole") || name.contains("cellar") {
        "trapdoor"
    } else if name.contains("portal") || name.contains("volcano") || name.contains("cave") {
        "portal"
    } else {
        "other"
    }
}

/// Clicks the server refuses from every side: kept in the table so the
/// planner can skip them, with the reason why.
fn disabled_reason(from_tile: (i32, i32, i32), to_tile: (i32, i32, i32)) -> Option<&'static str> {
    match (from_tile, to_tile) {
        ((0, 3046, 3483), (1, 3046, 3483)) | ((0, 3057, 3483), (1, 3057, 3483)) => {
            Some("Prayer Guild ladder: members of the order only (%prayer_guild)")
        }
        ((0, 3111, 9526), (0, 3111, 3126)) | ((0, 3088, 3119), (0, 3088, 9519)) => {
            Some("Tutorial Island ladder: tutorial progress gate")
        }
        ((0, 3302, 3116), (0, 3302, 3119)) => Some("Shantay Pass gate: members only, not a level change"),
        _ => None,
    }
}

fn loc_names(gametypes: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(gametypes).map_err(|err| format!("read {}: {err}", gametypes.display()))?;
    let parsed = json::parse(&text).map_err(|err| format!("parse {}: {err}", gametypes.display()))?;
    let rows = parsed.get("loc").as_arr().ok_or_else(|| format!("{} has no loc table", gametypes.display()))?;
    Ok(rows.iter().map(|row| row.get("name").as_str().unwrap_or("").to_lowercase()).collect())
}

/// Build the table and write it to `output`. Returns (rows, skipped doors).
pub fn dump_transitions(output: &Path) -> Result<(usize, usize), String> {
    let content = content_dir();
    let engine = engine_dir();
    let pack_dir = engine.join("data").join("pack");
    let gametypes = gametypes_path();
    let defs = crate::staticmap::load_loctypes(&gametypes)?;
    let names = loc_names(&gametypes)?;
    let zones = f2p_zones(&content)?;
    let handled = handlers(&content)?;
    let spots = placements(&pack_dir, &zones)?;

    let mut by_name: BTreeMap<String, Vec<Spot>> = BTreeMap::new();
    for spot in spots {
        let name = names.get(spot.loc_id as usize).cloned().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        by_name.entry(name).or_default().push(spot);
    }

    let mut id_of_name: HashMap<String, i32> = HashMap::new();
    for (name, group) in &by_name {
        id_of_name.entry(name.clone()).or_insert(group[0].loc_id);
    }
    for (loc_id, name) in names.iter().enumerate() {
        if !name.is_empty() {
            id_of_name.entry(name.clone()).or_insert(loc_id as i32);
        }
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut seen: HashSet<((i32, i32, i32), (i32, i32, i32), i32, u8)> = HashSet::new();
    let mut skipped: Vec<(String, u8, (i32, i32, i32), (i32, i32, i32))> = Vec::new();
    for (name, group) in &by_name {
        let mut ops: Vec<u8> = handled.keys().filter(|(candidate, _)| candidate == name).map(|(_, op)| *op).collect();
        ops.sort_unstable();
        if ops.is_empty() {
            continue;
        }
        for spot in group {
            let from_tile = (spot.level, spot.tile_x, spot.tile_z);
            for op in &ops {
                let (target, delta, climb_loc, climb_op, open_to) = if let Some((opened, next_op, found)) =
                    opened_to(name, *op, &handled, from_tile, spot.angle)
                {
                    let (target, delta) = found;
                    let Some(target) = target else { continue };
                    if target == from_tile {
                        continue;
                    }
                    (target, delta, id_of_name.get(&opened).copied().unwrap_or(spot.loc_id), next_op, id_of_name.get(&opened).copied())
                } else {
                    let Some(found) = resolved(name, *op, &handled, from_tile, spot.angle) else {
                        continue;
                    };
                    let (target, delta) = found;
                    let Some(target) = target else { continue };
                    if target == from_tile {
                        continue;
                    }
                    (target, delta, spot.loc_id, *op, None)
                };
                if !(0..4).contains(&target.0) {
                    continue;
                }
                if target.0 == from_tile.0 && (target.1 - spot.tile_x).abs() + (target.2 - spot.tile_z).abs() <= 2 {
                    skipped.push((name.clone(), *op, from_tile, target));
                    continue;
                }
                let key = (from_tile, target, climb_loc, climb_op);
                if !seen.insert(key) {
                    continue;
                }
                rows.push(Row {
                    name: format!("{name}_{}", rows.len()),
                    kind: kind_of(name).to_string(),
                    from_tile,
                    to_tile: target,
                    loc: spot.loc_id,
                    climb_loc,
                    climb_op,
                    open_to,
                    delta,
                    footprint: footprint_tiles(&defs, spot.level, spot.tile_x, spot.tile_z, spot.loc_id, spot.shape, spot.angle),
                    disabled: disabled_reason(from_tile, target),
                });
            }
        }
    }

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let footprint: Vec<Value> = row
            .footprint
            .iter()
            .map(|(tile_x, tile_z)| Value::Arr(vec![Value::Int(i64::from(*tile_x)), Value::Int(i64::from(*tile_z))]))
            .collect();
        items.push(Value::Obj(
            [
                ("climb_loc".to_string(), Value::Int(i64::from(row.climb_loc))),
                ("climb_op".to_string(), Value::Int(i64::from(row.climb_op))),
                (
                    "delta".to_string(),
                    row.delta.map(|(dlevel, dx, dz)| {
                        Value::Arr(vec![Value::Int(i64::from(dlevel)), Value::Int(i64::from(dx)), Value::Int(i64::from(dz))])
                    }).unwrap_or(Value::Null),
                ),
                (
                    "disabled".to_string(),
                    row.disabled.map(|reason| Value::Str(reason.to_string())).unwrap_or(Value::Null),
                ),
                ("footprint".to_string(), Value::Arr(footprint)),
                (
                    "from".to_string(),
                    Value::Arr(vec![
                        Value::Int(i64::from(row.from_tile.0)),
                        Value::Int(i64::from(row.from_tile.1)),
                        Value::Int(i64::from(row.from_tile.2)),
                    ]),
                ),
                ("kind".to_string(), Value::Str(row.kind.clone())),
                ("loc".to_string(), Value::Int(i64::from(row.loc))),
                ("name".to_string(), Value::Str(row.name.clone())),
                (
                    "open_to".to_string(),
                    row.open_to.map(|loc| Value::Int(i64::from(loc))).unwrap_or(Value::Null),
                ),
                (
                    "to".to_string(),
                    Value::Arr(vec![
                        Value::Int(i64::from(row.to_tile.0)),
                        Value::Int(i64::from(row.to_tile.1)),
                        Value::Int(i64::from(row.to_tile.2)),
                    ]),
                ),
            ]
            .into_iter()
            .collect(),
        ));
    }
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
        }
    }
    std::fs::write(output, json::stringify_pretty(&Value::Arr(items)))
        .map_err(|err| format!("write {}: {err}", output.display()))?;
    for (name, op, from_tile, target) in &skipped {
        println!("  skipped {name} op{op} {from_tile:?} -> {target:?}");
    }
    println!("transitions={} skipped={} -> {}", rows.len(), skipped.len(), output.display());
    Ok((rows.len(), skipped.len()))
}

#[cfg(test)]
mod tests {
    use super::dump_transitions;
    use crate::json;
    use crate::paths;
    use std::collections::HashSet;

    fn triple(value: &json::Value) -> (i64, i64, i64) {
        let items = value.as_arr().expect("triple is an array");
        (items[0].as_i64().unwrap(), items[1].as_i64().unwrap(), items[2].as_i64().unwrap())
    }

    fn sort_key(row: &json::Value) -> ((i64, i64, i64), i64, i64) {
        (triple(row.get("from")), row.get("loc").as_i64().unwrap(), row.get("climb_op").as_i64().unwrap())
    }

    fn norm_footprint(row: &json::Value) -> Vec<(i64, i64)> {
        let mut tiles: Vec<(i64, i64)> = row
            .get("footprint")
            .as_arr()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tile| {
                let pair = tile.as_arr().expect("footprint tile is a pair");
                (pair[0].as_i64().unwrap(), pair[1].as_i64().unwrap())
            })
            .collect();
        tiles.sort_unstable();
        tiles
    }

    #[test]
    fn output_matches_committed_table() {
        let reference_path = paths::root().join("lifescape").join("data").join("transitions.json");
        let reference_text = std::fs::read_to_string(&reference_path).expect("read committed transitions");
        let out_path = std::env::temp_dir().join(format!("transitions-test-{}.json", std::process::id()));
        let (rows, _skipped) = dump_transitions(&out_path).expect("dump transitions");
        let produced_text = std::fs::read_to_string(&out_path).expect("read dumped transitions");
        let _ = std::fs::remove_file(&out_path);
        let mut reference = json::parse(&reference_text).expect("parse committed").as_arr().cloned().unwrap_or_default();
        let mut produced = json::parse(&produced_text).expect("parse dumped").as_arr().cloned().unwrap_or_default();
        assert_eq!(produced.len(), reference.len(), "row count drifted");
        assert_eq!(rows, reference.len(), "reported count matches file");
        reference.sort_by_key(sort_key);
        produced.sort_by_key(sort_key);
        let mut seen_keys = HashSet::new();
        for row in &produced {
            assert!(seen_keys.insert(sort_key(row)), "duplicate row {row:?}");
        }
        for (index, (want, got)) in reference.iter().zip(produced.iter()).enumerate() {
            for key in ["kind", "from", "to", "loc", "climb_loc", "climb_op", "open_to", "delta", "disabled"] {
                assert_eq!(got.get(key), want.get(key), "row {index} field {key} drifted: {got:?} vs {want:?}");
            }
            assert_eq!(norm_footprint(got), norm_footprint(want), "row {index} footprint drifted");
        }
    }
}
