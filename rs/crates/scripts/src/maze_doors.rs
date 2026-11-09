//! Maze walllow doors: entry sides, stand order, swing clicks and the
//! maze-aware stepping that goes with them. A script event's doors, not
//! generic SDK doors: the generic router never plans through the maze,
//! so only this solver walks here.
//!
//! Mapless: no painted collision, no painted door registry, no local
//! router. Each leg fetches its tiles in one `tiles_flags` batch (one
//! round trip per leg, never one per step), off-plan doors resolve
//! through `door_defs`, and the step the server accepts is the only
//! verification. A down service fails loud, never a silent stall.
//!
//! Moved from `sdk/src/client/doors.rs` (phase 8.2): same logic, free
//! functions over `&mut Bot` instead of methods (an external crate cannot
//! add inherent impls). The SDK keeps only generic doors.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_nav::service_queries::TileFlags;
use lifescape_sdk::collision::CollisionMap;
use lifescape_sdk::error::{Error, Result};
use lifescape_sdk::Bot;

/// Entry side of a maze walllow door loc (content `all.loc`, category
/// `macro_maze_wall_door`): which cardinal stands swing through.
pub fn maze_entry_side(loc_id: u16) -> Option<u8> {
    match loc_id {
        3628 => Some(0), // macro_maze_walllow
        3629 => Some(1), // macro_maze_walllow_safe1
        3630 => Some(2), // macro_maze_walllow_safe2
        3631 => Some(2), // macro_maze_walllow_safe3
        3632 => Some(1), // macor_maze_walllow_safe4
        _ => None,
    }
}

/// True for the five maze walllow doors (content `all.loc`, category
/// `macro_maze_wall_door`), the only maze locs that swing.
pub fn is_maze_door(loc_id: u16) -> bool {
    maze_entry_side(loc_id).is_some()
}

/// Axis stands of a straight-wall maze door (door_procs.rs2
/// check_axis): angles 0/2 read same-X tiles (N/S), 1/3 same-Z (E/W).
pub fn maze_axis_stands(angle: u8, door_x: i32, door_z: i32) -> Vec<(i32, i32)> {
    match angle {
        0 | 2 => vec![(door_x, door_z + 1), (door_x, door_z - 1)],
        _ => vec![(door_x + 1, door_z), (door_x - 1, door_z)],
    }
}

/// Off-axis stands (the E/O neighbours for a N/S door and vice versa):
/// fallback when every axis stand fails, never the first try.
pub fn maze_off_stands(angle: u8, door_x: i32, door_z: i32) -> Vec<(i32, i32)> {
    match angle {
        0 | 2 => vec![(door_x + 1, door_z), (door_x - 1, door_z)],
        _ => vec![(door_x, door_z + 1), (door_x, door_z - 1)],
    }
}

/// Angle driving the side choice: the live one when the walllow still
/// stands (same id), else the static registry paint. A mid-swing
/// inviswall or an unstreamed tile must not steer the stands.
pub fn maze_effective_angle(
    live: Option<(u16, u8, u8)>,
    loc_id: u16,
    static_angle: u8,
) -> (u8, &'static str) {
    match live {
        Some((live_id, _, live_angle)) if live_id == loc_id => (live_angle, "live"),
        _ => (static_angle, "static"),
    }
}

/// Full stand order for one approach call, local only: the entry-side
/// group first (axis, except entry 2 which wants the off side), then the
/// other group as fallback. Each group sorts by Manhattan distance to
/// the bot. No memory between calls: the solver `tried` set stays the
/// only blacklist.
pub fn maze_approach_order(
    entry_side: u8,
    angle: u8,
    door_x: i32,
    door_z: i32,
    bot_x: i32,
    bot_z: i32,
) -> Vec<(i32, i32)> {
    let mut preferred = if entry_side == 2 {
        maze_off_stands(angle, door_x, door_z)
    } else {
        maze_axis_stands(angle, door_x, door_z)
    };
    let mut fallback = if entry_side == 2 {
        maze_axis_stands(angle, door_x, door_z)
    } else {
        maze_off_stands(angle, door_x, door_z)
    };
    preferred.sort_by_key(|stand| (stand.0 - bot_x).abs() + (stand.1 - bot_z).abs());
    fallback.sort_by_key(|stand| (stand.0 - bot_x).abs() + (stand.1 - bot_z).abs());
    preferred.extend(fallback);
    preferred
}

/// One diagnostic line logged before every maze door click: the exact
/// live loc at the tile (`loc id shape angle`, or `none` when nothing
/// streamed — same wording as `refused_line`), the bot position, and the
/// chosen stand with why (entry, angle source, axis). Pure so tests lock
/// the wording.
pub fn maze_door_diag(
    door_x: i32,
    door_z: i32,
    door_level: i32,
    live: Option<(u16, u8, u8)>,
    bot_x: i32,
    bot_z: i32,
    bot_level: i32,
    entry_side: u8,
    angle: u8,
    angle_source: &str,
    stand_x: i32,
    stand_z: i32,
) -> String {
    let live_text = match live {
        Some((live_id, live_shape, live_angle)) => {
            format!("loc {live_id} shape={live_shape} angle={live_angle}")
        }
        None => "none".to_string(),
    };
    let on_axis = match angle {
        0 | 2 => stand_x == door_x,
        _ => stand_z == door_z,
    };
    let wanted = entry_side == 0 || (entry_side == 1 && on_axis) || (entry_side == 2 && !on_axis);
    let reason = if entry_side == 0 {
        "entry 0 accepts every side, axis preferred"
    } else if wanted {
        "entry side, click swings through"
    } else {
        "off-side fallback, parks on door then through"
    };
    format!(
        "maze door ({door_x},{door_z},{door_level}) live={live_text} from ({bot_x},{bot_z},{bot_level}) stand=({stand_x},{stand_z}) entry={entry_side} angle={angle}({angle_source}) axis={on_axis} {reason}"
    )
}

/// Diamond window tiles around a center tile, for one batch read.
pub fn window_tiles(center_x: i32, center_z: i32, level: i32, radius: i32) -> Vec<(i32, i32, i32)> {
    let radius = radius.max(0);
    let mut tiles = Vec::new();
    for scan_x in center_x - radius..=center_x + radius {
        let rest = radius - (scan_x - center_x).abs();
        for scan_z in center_z - rest..=center_z + rest {
            tiles.push((scan_x, scan_z, level));
        }
    }
    tiles
}

/// Scratch map plus door index off one batch reply: flags drive the
/// shared-edge step test, a recorded door (else a maze static loc) names
/// the door per tile. Ground decor never lands here: `loc` only counts
/// for maze walllow ids. Pure: the worker map stays empty, the scratch
/// dies with the leg.
pub fn scratch_from_rows(rows: &[TileFlags]) -> (CollisionMap, HashMap<(i32, i32, i32), (u16, u8, u8)>) {
    let mut scratch = CollisionMap::new();
    let mut doors = HashMap::new();
    for row in rows {
        scratch.set(row.tile.x, row.tile.z, row.tile.level, row.flags);
        let tile = (row.tile.x, row.tile.z, row.tile.level);
        if let Some(triple) = row.door {
            doors.insert(tile, triple);
        } else if let Some(triple) = row.loc {
            if is_maze_door(triple.0) {
                doors.insert(tile, triple);
            }
        }
    }
    (scratch, doors)
}

/// Shortest cardinal path on a scratch map. Unknown tiles read shut, so
/// the search stays inside the batched window. Capped: the approach only
/// walks a few tiles per stand.
fn bfs_steps(scratch: &CollisionMap, level: i32, from: (i32, i32), to: (i32, i32)) -> Option<Vec<(i32, i32)>> {
    use std::collections::VecDeque;
    if from == to {
        return Some(Vec::new());
    }
    let mut parent: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    let mut seen: HashSet<(i32, i32)> = HashSet::from([from]);
    let mut queue = VecDeque::from([from]);
    let mut expanded = 0usize;
    while let Some(current) = queue.pop_front() {
        expanded += 1;
        if expanded > 600 {
            return None;
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            if !lifescape_sdk::collision::step_open(scratch, current.0, current.1, level, dx, dz) {
                continue;
            }
            let next = (current.0 + dx, current.1 + dz);
            if seen.contains(&next) {
                continue;
            }
            seen.insert(next);
            parent.insert(next, current);
            if next == to {
                let mut path = vec![next];
                let mut at = current;
                while at != from {
                    path.push(at);
                    at = parent[&at];
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(next);
        }
    }
    None
}

/// One refusal line, same shape as the walk legs: the wanted tile, the
/// start, the live loc there and the batch flags (`unknown` when the row
/// missed). Pure so tests lock the wording.
fn maze_refused_line(wanted: (i32, i32), start: (i32, i32, i32), live: Option<(u16, u8, u8)>, flags: Option<u32>) -> String {
    let live_text = match live {
        Some((loc_id, shape, angle)) => format!("loc {loc_id} shape={shape} angle={angle}"),
        None => "none".to_string(),
    };
    let flags_text = match flags {
        Some(flags) => format!("{flags:#x}"),
        None => "unknown".to_string(),
    };
    format!(
        "step refused: wanted [{wanted:?}] from ({},{},{}) live={live_text} flags={flags_text}",
        start.0, start.1, start.2
    )
}

/// Stand on a cardinal tile of a maze door, on its entry side: the
/// server refuses wall clicks from diagonals ("can't reach that")
/// and one-way doors from the wrong side, and a click from the wrong
/// side only bounces back onto the door. True once standing on one.
/// One batch window around the bot feeds a scratch BFS per stand; every
/// step is server truth, a refused stand tries the next one.
fn approach_maze_door(
    bot: &mut Bot,
    door_x: i32,
    door_z: i32,
    door_level: i32,
    loc_id: u16,
    angle: u8,
) -> Result<bool> {
    let entry = maze_entry_side(loc_id).unwrap_or(0);
    // Live angle steers when the walllow still stands; a mid-swing
    // inviswall or an unstreamed tile keeps the batch angle.
    let live_here = bot.world.loc_at(door_x, door_z, door_level);
    let (eff_angle, angle_source) = maze_effective_angle(live_here, loc_id, angle);
    // Preferred group first, off-axis fallback after: an axis stand
    // that never moves (or an op the server refuses) still leaves the
    // E/O neighbours to try in the same call. Bounded walk legs only.
    let ordered = maze_approach_order(entry, eff_angle, door_x, door_z, bot.world.x, bot.world.z);
    bot.log(&format!(
        "maze approach door ({door_x},{door_z}) live={} from {:?} entry={entry} angle={eff_angle}({angle_source}) order={ordered:?}",
        match live_here {
            Some((live_id, live_shape, live_angle)) => format!("loc {live_id} shape={live_shape} angle={live_angle}"),
            None => "none".to_string(),
        },
        bot.position(),
    ));
    // The only static read: one window batch. Down service fails loud,
    // never a blind walk.
    let rows = bot.tiles_flags(&window_tiles(bot.world.x, bot.world.z, door_level, 10)).map_err(|error| {
        bot.log(&format!("maze approach door ({door_x},{door_z}) static batch down: {error}"));
        error
    })?;
    let (scratch, _) = scratch_from_rows(&rows);
    for (stand_x, stand_z) in ordered {
        if bot.world.level == door_level && (bot.world.x, bot.world.z) == (stand_x, stand_z) {
            return Ok(true);
        }
        if bot.world.level != door_level {
            break;
        }
        let Some(path) = bfs_steps(&scratch, door_level, (bot.world.x, bot.world.z), (stand_x, stand_z)) else {
            continue;
        };
        if path.len() > 12 {
            continue;
        }
        let mut arrived = true;
        for (step_x, step_z) in path {
            match bot.step_to(step_x, step_z, 8) {
                Ok(true) => {}
                Ok(false) => {
                    arrived = false;
                    break;
                }
                Err(error) if lifescape_sdk::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) => {
                    return Err(error);
                }
                Err(_) => {
                    arrived = false;
                    break;
                }
            }
        }
        if arrived && bot.world.level == door_level && (bot.world.x, bot.world.z) == (stand_x, stand_z) {
            return Ok(true);
        }
    }
    // Already cardinally adjacent (or on the door): click from here.
    Ok(bot.world.level == door_level && (bot.world.x - door_x).abs() + (bot.world.z - door_z).abs() <= 1)
}

/// Nearest known maze walllow door within `radius` tiles, if any. Reads
/// the ephemeral run registry, never painted state.
pub fn nearest_maze_door(bot: &Bot, radius: i32) -> Option<(i32, i32, i32)> {
    let mut best: Option<(i32, (i32, i32, i32))> = None;
    for (tile, (loc_id, _, _)) in bot.doors.iter() {
        if tile.2 != bot.world.level || !is_maze_door(*loc_id) {
            continue;
        }
        let dist = (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs();
        if dist <= radius && best.is_none_or(|known| dist < known.0) {
            best = Some((dist, *tile));
        }
    }
    best.map(|(_, tile)| tile)
}

/// Open one maze walllow door: stand on its entry side, click, and if the
/// swing parks us on the door tile, click once more from there
/// (standing on the door reads as the through side, so the second
/// click always teleports through). True when the server responded
/// (parked or through: either way replan from the new tile); only a
/// door that leaves us exactly in place reads shut and blacklists.
pub fn open_maze_door(bot: &mut Bot, door_x: i32, door_z: i32, door_level: i32) -> Result<bool> {
    // Door identity: live first (a mid-swing inviswall differs from the
    // record), else one batch tile. Unknown or non-maze reads fail loud,
    // never a blind click.
    let (loc_id, shape, angle) = match bot.world.loc_at(door_x, door_z, door_level) {
        Some(triple) if is_maze_door(triple.0) => triple,
        _ => {
            let rows = bot.tiles_flags(&[(door_x, door_z, door_level)]).map_err(|error| {
                bot.log(&format!("maze door ({door_x},{door_z}) identity batch down: {error}"));
                error
            })?;
            match rows.into_iter().next().and_then(|row| row.door.or(row.loc)).filter(|triple| is_maze_door(triple.0)) {
                Some(triple) => triple,
                None => return Err(Error::Unreachable(format!("no known maze door at ({door_x},{door_z},{door_level})"))),
            }
        }
    };
    // The click comes from the service def, never a guess: no open action
    // reads shut and loud, a down service propagates its typed error.
    let open_op = bot
        .door_defs(std::slice::from_ref(&loc_id))
        .map_err(|error| {
            bot.log(&format!("maze door loc {loc_id} at ({door_x},{door_z}) defs down: {error}"));
            error
        })?
        .into_iter()
        .next()
        .and_then(|def| def.open_op);
    let Some(op) = open_op else {
        bot.log(&format!("maze door loc {loc_id} at ({door_x},{door_z}) has no open action, leaving it shut"));
        return Ok(false);
    };
    if !approach_maze_door(bot, door_x, door_z, door_level, loc_id, angle)? {
        bot.log(&format!(
            "maze door {loc_id} at ({door_x},{door_z}) unreachable from {:?}, leaving it shut",
            bot.position()
        ));
        return Ok(false);
    }
    bot.log(&format!("opening maze door loc {loc_id} at ({door_x},{door_z},{door_level})"));
    bot.intend(&format!("opening the maze door at ({door_x},{door_z})"));
    let start = (bot.world.x, bot.world.z);
    // Through-destination of the swing (door_open for straight walls).
    let dest = match (shape, angle) {
        (0, 0) => (door_x - 1, door_z),
        (0, 1) => (door_x, door_z + 1),
        (0, 2) => (door_x + 1, door_z),
        (0, 3) => (door_x, door_z - 1),
        _ => (door_x, door_z),
    };
    // Up to three clicks: an off-side first click parks us on the
    // door tile, and the swing turns the loc to inviswall for a few
    // ticks (open_and_close_door) — a click inside that window is
    // ignored, so wait for the walllow back before clicking again.
    for _ in 0..3 {
        let before = (bot.world.x, bot.world.z);
        // Live diagnostic at click time: exact loc, position, chosen
        // side and why. A refused op ("can't reach that") or a bot
        // that never moves reads here first.
        let live_now = bot.world.loc_at(door_x, door_z, door_level);
        let entry_now = maze_entry_side(loc_id).unwrap_or(0);
        let (eff_now, source_now) = maze_effective_angle(live_now, loc_id, angle);
        bot.log(&maze_door_diag(
            door_x, door_z, door_level, live_now,
            bot.world.x, bot.world.z, bot.world.level,
            entry_now, eff_now, source_now, bot.world.x, bot.world.z,
        ));
        if bot.op_loc(op, door_x, door_z, loc_id).is_err() {
            break;
        }
        let target = bot.world.ticks + 6;
        while bot.world.ticks < target {
            if bot.pump(1).is_err() {
                break;
            }
            let now = (bot.world.x, bot.world.z);
            if now == dest && now != before {
                bot.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
                return Ok(true);
            }
            if now == (door_x, door_z) {
                break;
            }
        }
        if (bot.world.x, bot.world.z) != (door_x, door_z) {
            break;
        }
        let revert = bot.world.ticks + 8;
        while bot.world.ticks < revert {
            if bot.pump(1).is_err() {
                break;
            }
            if matches!(bot.world.loc_at(door_x, door_z, door_level), Some((id, _, _)) if id == loc_id) {
                break;
            }
        }
    }
    bot.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
    if (bot.world.x, bot.world.z) != start {
        // Parked or through without matching the computed dest (stale
        // angle): the door responded, replan from here instead of
        // blacklisting a working door.
        return Ok(true);
    }
    bot.log(&format!("maze door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut"));
    if bot.world.modal.as_ref().is_some_and(|modal| modal.layer != "overlay") {
        let _ = bot.close_modal();
        let _ = bot.pump(2);
    }
    Ok(false)
}

/// Walk explicit steps one tile at a time inside the maze, swinging shut
/// doors that block the way. One `tiles_flags` batch covers the whole
/// leg (start plus every step, deduped): a single round trip per leg,
/// never one per step. Shut edges swing a maze door when the batch names
/// one there (ordinary shut doors go through the generic opener), and
/// anything else stops at once so the caller replans instead of
/// detouring blindly. Every step is server truth; a down service fails
/// loud before the first step. True when every step is done (a teleport
/// mid-route stops early for the same reason).
pub fn walk_maze_steps(bot: &mut Bot, steps: &[(i32, i32)], timeout: u64) -> Result<bool> {
    if steps.is_empty() {
        return Ok(true);
    }
    let deadline = bot.world.ticks + timeout.max(1);
    let level = bot.world.level;
    let mut tiles = vec![(bot.world.x, bot.world.z, level)];
    tiles.extend(steps.iter().map(|(step_x, step_z)| (*step_x, *step_z, level)));
    tiles.sort();
    tiles.dedup();
    let rows = bot.tiles_flags(&tiles).map_err(|error| {
        bot.log(&format!("maze walk batch down at {:?}: {error}", bot.position()));
        error
    })?;
    let (scratch, doors) = scratch_from_rows(&rows);
    let flag_of = |tile: (i32, i32)| {
        rows.iter()
            .find(|row| (row.tile.x, row.tile.z) == tile && row.tile.level == level)
            .map(|row| row.flags)
    };
    let mut index = 0;
    while index < steps.len() {
        if bot.world.ticks > deadline {
            return Err(Error::Timeout(format!(
                "walk_path timed out at {:?} ({} ticks)",
                bot.position(),
                timeout
            )));
        }
        let (step_x, step_z) = steps[index];
        if (bot.world.x, bot.world.z) == (step_x, step_z) {
            index += 1;
            continue;
        }
        if bot.world.level != level {
            // Teleport mid-route (a door swing lands elsewhere): stop so
            // the caller replans from the new tile.
            return Ok(false);
        }
        let (dx, dz) = (step_x - bot.world.x, step_z - bot.world.z);
        // Cardinal legs for this step, open edge first.
        let mut legs: Vec<(i32, i32)> = Vec::new();
        if dx != 0 && dz != 0 {
            let x_open = lifescape_sdk::collision::step_open(&scratch, bot.world.x, bot.world.z, level, dx.signum(), 0);
            let z_open = lifescape_sdk::collision::step_open(&scratch, bot.world.x, bot.world.z, level, 0, dz.signum());
            if x_open {
                legs.push((dx.signum(), 0));
                legs.push((0, dz.signum()));
            } else if z_open {
                legs.push((0, dz.signum()));
                legs.push((dx.signum(), 0));
            }
        } else if dx != 0 || dz != 0 {
            legs.push((dx.signum(), dz.signum()));
        } else {
            index += 1;
            continue;
        }
        if legs.is_empty() {
            // Corner shut on both edges: swing a maze door there if the
            // batch names one, then replan from wherever that leaves us.
            for (tile_x, tile_z) in [(bot.world.x + dx.signum(), bot.world.z), (bot.world.x, bot.world.z + dz.signum())] {
                if let Some((loc_id, _, _)) = doors.get(&(tile_x, tile_z, level)).copied() {
                    if is_maze_door(loc_id) {
                        match open_maze_door(bot, tile_x, tile_z, level) {
                            Ok(_) => {}
                            Err(error) if lifescape_sdk::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) => {
                                return Err(error);
                            }
                            Err(error) => {
                                bot.log(&format!("maze walk corner door at ({tile_x},{tile_z}) failed loud: {error}"));
                            }
                        }
                    }
                    break;
                }
            }
            bot.log(&format!(
                "maze walk corner shut at {:?} wanted ({step_x},{step_z}), replanning",
                bot.position()
            ));
            return Ok(false);
        }
        let mut advanced = false;
        for (move_x, move_z) in legs {
            if (bot.world.x, bot.world.z) == (step_x, step_z) {
                break;
            }
            if bot.world.level != level {
                return Ok(false);
            }
            let (next_x, next_z) = (bot.world.x + move_x, bot.world.z + move_z);
            // A recorded door the live stream still shows closed clicks
            // first even when the static edge reads open: flags never
            // carry live door states, the batch only names the door.
            let door_record = doors.get(&(next_x, next_z, level)).copied();
            let live_shut = match (door_record, bot.world.loc_at(next_x, next_z, level)) {
                (Some((loc_id, _, _)), Some((live_id, _, _))) => live_id == loc_id,
                _ => false,
            };
            if !lifescape_sdk::collision::step_open(&scratch, bot.world.x, bot.world.z, level, move_x, move_z) || live_shut {
                match door_record {
                    Some((loc_id, _, _)) if is_maze_door(loc_id) => {
                        match open_maze_door(bot, next_x, next_z, level) {
                            Ok(_) => {}
                            Err(error) if lifescape_sdk::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) => {
                                return Err(error);
                            }
                            Err(error) => {
                                bot.log(&format!("maze walk door at ({next_x},{next_z}) failed loud: {error}"));
                            }
                        }
                        break;
                    }
                    Some((loc_id, _, _)) => {
                        match bot.open_shut_door(next_x, next_z, level, loc_id) {
                            Ok(true) => {}
                            Ok(false) => {
                                bot.log(&format!(
                                    "maze walk door loc {loc_id} at ({next_x},{next_z}) stays shut from {:?}, replanning",
                                    bot.position()
                                ));
                                break;
                            }
                            Err(error)
                                if lifescape_sdk::error::is_interrupted(&error)
                                    || matches!(error, Error::Io(_) | Error::Login(_)) =>
                            {
                                return Err(error)
                            }
                            Err(error) => {
                                bot.log(&format!("maze walk door loc {loc_id} at ({next_x},{next_z}) failed loud: {error}"));
                                break;
                            }
                        }
                    }
                    None => {
                        let live = bot.world.loc_at(next_x, next_z, level);
                        bot.log(&maze_refused_line((next_x, next_z), (bot.world.x, bot.world.z, level), live, flag_of((next_x, next_z))));
                        break;
                    }
                }
            }
            match bot.step_to(next_x, next_z, 8) {
                Ok(true) => {
                    advanced = true;
                }
                Ok(false) => {
                    let live = bot.world.loc_at(next_x, next_z, level);
                    bot.log(&maze_refused_line((next_x, next_z), (bot.world.x, bot.world.z, level), live, flag_of((next_x, next_z))));
                    break;
                }
                Err(error) if lifescape_sdk::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) => return Err(error),
                Err(_) => break,
            }
        }
        if (bot.world.x, bot.world.z) == (step_x, step_z) {
            index += 1;
        } else if !advanced {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Served bot: real dumps wired for the snapshot, one plan socket.
    /// Game sends still fail typed (dead game socket), service reads pass.
    fn served_bot(account: &str, socket: &std::path::Path, version: &str) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, version.to_string()),
            socket,
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), account));
        bot.plan_data_version = Some(version.to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        (bot, service)
    }

    /// Stand choice follows the live angle, not the batch one: a door
    /// registered angle 0 but streaming angle 1 wants E/W stands first.
    #[test]
    fn maze_stands_follow_live_angle_not_static() {
        let (live_angle, source) = maze_effective_angle(Some((3628, 0, 1)), 3628, 0);
        assert_eq!((live_angle, source), (1, "live"), "standing walllow steers");
        let order = maze_approach_order(0, live_angle, 2932, 4559, 2932, 4560);
        assert!(
            order.iter().take(2).all(|(stand_x, _)| *stand_x != 2932),
            "live angle 1 wants E/W first, got: {order:?}"
        );
        let (static_angle, static_source) = maze_effective_angle(None, 3628, 0);
        assert_eq!((static_angle, static_source), (0, "static"), "unstreamed tile keeps paint");
        let (mid_angle, mid_source) = maze_effective_angle(Some((4430, 0, 3)), 3628, 0);
        assert_eq!((mid_angle, mid_source), (0, "static"), "mid-swing inviswall steers nothing");
    }

    /// Axis blocked still leaves the E/O neighbours in the same call: the
    /// full order is preferred group first, fallback group after, four
    /// cardinal stands, no walk fan, no memory between calls.
    #[test]
    fn maze_approach_falls_back_to_off_axis_stands() {
        let order = maze_approach_order(0, 0, 2932, 4559, 2932, 4560);
        assert_eq!(order.len(), 4, "four cardinal stands, got: {order:?}");
        assert!(
            order.iter().take(2).all(|(stand_x, _)| *stand_x == 2932),
            "entry 0 angle 0 tries N/S first, got: {order:?}"
        );
        assert!(
            order.iter().skip(2).all(|(stand_x, _)| *stand_x != 2932),
            "E/O fallback follows in the same call, got: {order:?}"
        );
        let oneway = maze_approach_order(2, 0, 2932, 4559, 2932, 4560);
        assert!(
            oneway.iter().take(2).all(|(stand_x, _)| *stand_x != 2932),
            "entry 2 wants the off side first, got: {oneway:?}"
        );
        assert_eq!(oneway.len(), 4, "fallback keeps four stands");
    }

    /// Pre-click diagnostic names the exact live loc, the bot position and
    /// the chosen side with why: the next maze refusal reads here first.
    #[test]
    fn maze_door_diag_names_live_position_and_side() {
        let line = maze_door_diag(
            2932, 4559, 0,
            Some((3628, 0, 2)),
            2932, 4560, 0,
            0, 0, "live", 2932, 4560,
        );
        assert!(line.contains("live=loc 3628 shape=0 angle=2"), "exact live loc, got: {line}");
        assert!(line.contains("from (2932,4560,0)"), "bot position, got: {line}");
        assert!(line.contains("stand=(2932,4560)"), "chosen stand, got: {line}");
        assert!(line.contains("entry=0"), "entry reason, got: {line}");
        assert!(line.contains("angle=0(live)"), "angle source, got: {line}");
        assert!(line.contains("axis=true"), "axis flag, got: {line}");
        let missing = maze_door_diag(
            2932, 4559, 0, None,
            2932, 4560, 0,
            0, 0, "static", 2933, 4559,
        );
        assert!(missing.contains("live=none"), "unstreamed tile reads none, got: {missing}");
        assert!(missing.contains("axis=false"), "off-axis stand flags, got: {missing}");
    }

    /// Window tiles: diamond around the center, center included once, all
    /// on the level. One batch, no per-tile round trip.
    #[test]
    fn window_tiles_covers_diamond_once() {
        let tiles = window_tiles(100, 200, 0, 2);
        assert_eq!(tiles.len(), 13, "diamond radius 2 covers 13 tiles, got: {tiles:?}");
        assert!(tiles.contains(&(100, 200, 0)), "center included");
        assert!(tiles.iter().all(|(_, _, level)| *level == 0), "single level");
        let mut sorted = tiles.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), tiles.len(), "no duplicate tile");
    }

    /// Scratch off batch rows: flags land readable, recorded doors index
    /// (maze or ordinary), a maze static loc indexes too, ground decor
    /// never does.
    #[test]
    fn scratch_from_rows_names_recorded_doors_only() {
        use lifescape_nav::plan::Tile;
        let rows = vec![
            TileFlags { tile: Tile { x: 1, z: 1, level: 0 }, flags: 0x240100, door: Some((1530, 0, 2)), toll: false, loc: Some((1530, 0, 2)) },
            TileFlags { tile: Tile { x: 2, z: 1, level: 0 }, flags: 0, door: None, toll: false, loc: Some((3628, 0, 2)) },
            TileFlags { tile: Tile { x: 3, z: 1, level: 0 }, flags: 0, door: None, toll: false, loc: Some((695, 22, 0)) },
        ];
        let (scratch, doors) = scratch_from_rows(&rows);
        assert_eq!(scratch.get(1, 1, 0), 0x240100, "flags drive the step test");
        assert_eq!(doors.len(), 2, "recorded door plus maze static loc, got: {doors:?}");
        assert_eq!(doors.get(&(1, 1, 0)), Some(&(1530, 0, 2)), "ordinary recorded door indexes");
        assert_eq!(doors.get(&(2, 1, 0)), Some(&(3628, 0, 2)), "maze static loc indexes");
        assert!(!doors.contains_key(&(3, 1, 0)), "ground decor is no door");
    }

    /// No service, no steps: the leg batch is the first read, so the
    /// failure is a typed `nav-down` before anything pumps — never a
    /// silent stall on painted state.
    #[test]
    fn walk_without_service_is_nav_down_before_any_step() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        let err = walk_maze_steps(&mut bot, &[(0, 1)], 60).expect_err("dead service aborts typed");
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "typed travel error, got: {err}");
        assert_eq!((bot.world.x, bot.world.z), (0, 0), "never moved");
        assert_eq!(bot.world.ticks, 0, "failure pumps nothing");
    }

    /// Shut ordinary door on the leg: the batch names it, the generic
    /// opener runs it (dead game socket: the click never leaves), and the
    /// leg stops loud with no blind step. Exactly one flags batch per leg.
    #[test]
    fn shut_ordinary_door_stops_loud_without_blind_step() {
        let (mut bot, service) = served_bot("maze-shut", &lifescape_nav::service::test_socket("maze-shut"), "maze-shut");
        bot.world.x = 3226;
        bot.world.z = 3222;
        bot.world.locs.insert((3226, 3223, 0), Some((1530, 0, 2)));
        let (_, before) = service.query_stats();
        let outcome = walk_maze_steps(&mut bot, &[(3226, 3223)], 60).expect("shut door stops, never aborts");
        let (_, after) = service.query_stats();
        assert!(!outcome, "shut door stops the leg for a replan");
        assert_eq!((bot.world.x, bot.world.z), (3226, 3222), "no blind step onto the shut tile");
        assert_eq!(bot.world.ticks, 0, "refusal pumps nothing");
        assert_eq!(after - before, 1, "one flags batch per leg, never one per step");
    }

    /// Maze door click without a game connection: defs and approach run
    /// (service is up, the bot already stands), the dead game socket
    /// refuses the click, and the door reads shut and loud — never a
    /// local success. The record is kept for the re-close.
    #[test]
    fn maze_door_click_without_game_conn_stays_shut_loud() {
        let (mut bot, _service) = served_bot("maze-click", &lifescape_nav::service::test_socket("maze-click"), "maze-click");
        // On the first-choice axis stand: no approach steps, the dead
        // game socket refuses the click itself.
        bot.world.x = 10;
        bot.world.z = 1;
        bot.world.locs.insert((10, 0, 0), Some((3628, 0, 2)));
        let defs = bot.door_defs(&[3628]).expect("defs answer");
        assert!(defs[0].open_op.is_some(), "the maze door carries its click");
        assert!(!open_maze_door(&mut bot, 10, 0, 0).expect("dead game socket reads shut, never aborts"), "no conn means no swing");
        assert_eq!(bot.world.ticks, 0, "refusal pumps nothing");
        assert_eq!(bot.doors.get(&(10, 0, 0)), Some(&(3628, 0, 2)), "record kept for the re-close");
    }

    /// Unstreamed tile, no service: identity needs its batch tile, so the
    /// failure is `nav-down` — loud, typed, before any click.
    #[test]
    fn maze_door_identity_without_service_is_nav_down() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        let err = open_maze_door(&mut bot, 10, 0, 0).expect_err("dead service aborts typed");
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "typed travel error, got: {err}");
        assert_eq!(bot.world.ticks, 0, "failure pumps nothing");
    }

    /// Tile holding no maze door: rejected without any click — the static
    /// batch is the authority (live may show a mid-swing inviswall), and
    /// its silence reads `Unreachable`, never a blind click. Game socket
    /// untouched (the click never leaves), no pump.
    #[test]
    fn non_maze_tile_rejected_without_click() {
        let (mut bot, _service) = served_bot("maze-non", &lifescape_nav::service::test_socket("maze-non"), "maze-non");
        bot.world.x = 9;
        bot.world.z = 0;
        bot.world.locs.insert((10, 0, 0), Some((1530, 0, 2)));
        let err = open_maze_door(&mut bot, 10, 0, 0).expect_err("non-maze tile never clicks");
        assert!(matches!(&err, Error::Unreachable(_)), "typed unreachable, got: {err}");
        assert_eq!(bot.world.ticks, 0, "rejection pumps nothing");
        assert!(bot.doors.get(&(10, 0, 0)).is_none(), "unguessed door records nothing");
    }

    /// Static flags never carry live door states: the shut ordinary door
    /// tile (3226,3223) reads edge-open north/south off its batch flags,
    /// so the stepper must consult the live loc before trusting the
    /// scratch. Locks the `live_shut` branch against "simplification".
    #[test]
    fn static_flags_miss_live_door_state() {
        let mut scratch = CollisionMap::new();
        scratch.set(3226, 3222, 0, 0x1008);
        scratch.set(3226, 3223, 0, 0x1008);
        assert!(
            lifescape_sdk::collision::step_open(&scratch, 3226, 3222, 0, 0, 1),
            "batch flags alone read the shut door open"
        );
    }
}
