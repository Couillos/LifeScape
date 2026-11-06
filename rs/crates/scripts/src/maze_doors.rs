//! Maze walllow doors: entry sides, stand order, swing clicks and the
//! maze-aware stepping that goes with them. A script event's doors, not
//! generic SDK doors: the generic router never plans through the maze
//! (see `nav_overrides.json`), only this solver walks here.
//!
//! Moved from `sdk/src/client/doors.rs` (phase 8.2): same logic, free
//! functions over `&mut Bot` instead of methods (an external crate cannot
//! add inherent impls). The SDK keeps only generic doors.

#![forbid(unsafe_code)]

use lifescape_sdk::error::{Error, Result};
use lifescape_sdk::route::Router;
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

/// Stand on a cardinal tile of a maze door, on its entry side: the
/// server refuses wall clicks from diagonals ("can't reach that")
/// and one-way doors from the wrong side, and a click from the wrong
/// side only bounces back onto the door. True once standing on one.
/// Walks explicit BFS steps (capped): walk legs never open random doors,
/// they only step next door.
fn approach_maze_door(
    bot: &mut Bot,
    door_x: i32,
    door_z: i32,
    door_level: i32,
    loc_id: u16,
    angle: u8,
    _timeout: u64,
) -> bool {
    let entry = maze_entry_side(loc_id).unwrap_or(0);
    // Live angle steers when the walllow still stands; a mid-swing
    // inviswall or an unstreamed tile keeps the static paint.
    let live_here = bot.world.loc_at(door_x, door_z, door_level);
    let (eff_angle, angle_source) = maze_effective_angle(live_here, loc_id, angle);
    // Preferred group first, off-axis fallback after: an axis stand
    // that never moves (or an op the server refuses) still leaves the
    // E/O neighbours to try in the same call. Bounded walk legs only.
    let ordered = maze_approach_order(entry, eff_angle, door_x, door_z, bot.world.x, bot.world.z);
    {
        let live_text = match live_here {
            Some((live_id, live_shape, live_angle)) => {
                format!("loc {live_id} shape={live_shape} angle={live_angle}")
            }
            None => "none".to_string(),
        };
        bot.log(&format!(
            "maze approach door ({door_x},{door_z}) live={live_text} from {:?} entry={entry} angle={eff_angle}({angle_source}) order={ordered:?}",
            bot.position()
        ));
    }
    bot.ensure_area(bot.world.x, bot.world.z, 70);
    let planning = match &bot.maps {
        Some(pack) => pack.planning_view(&bot.collision, &bot.doors, &bot.toll_gates, bot.allow_paid),
        None => bot.collision.clone(),
    };
    for (stand_x, stand_z) in ordered {
        if bot.world.level == door_level && (bot.world.x, bot.world.z) == (stand_x, stand_z) {
            return true;
        }
        let mut router = Router::new();
        let waypoints = router.find_path_to_tile(&planning, door_level, bot.world.x, bot.world.z, stand_x, stand_z);
        if waypoints.is_empty() {
            eprintln!("{} maze approach door ({door_x},{door_z}) stand ({stand_x},{stand_z}): no planned path", bot.tag());
            continue;
        }
        let steps = lifescape_sdk::nav::expand_waypoints(&waypoints, bot.world.x, bot.world.z);
        if steps.len() > 12 {
            eprintln!("{} maze approach door ({door_x},{door_z}) stand ({stand_x},{stand_z}): path too long", bot.tag());
            continue;
        }
        let _ = walk_maze_steps(bot, &steps, 60);
        eprintln!("{} maze approach door ({door_x},{door_z}) stand ({stand_x},{stand_z}): walked to {:?}", bot.tag(), bot.position());
        if bot.world.level == door_level && (bot.world.x, bot.world.z) == (stand_x, stand_z) {
            return true;
        }
    }
    // Exact stands can read shut in live collision while a plain
    // neighbour is one step away: the server walks the last step
    // itself on click (p_walk in macro_event_maze.rs2), so any
    // cardinal tile clicks. Prefer the entry side for one-way doors.
    let mut neighbours = vec![
        (door_x + 1, door_z),
        (door_x - 1, door_z),
        (door_x, door_z + 1),
        (door_x, door_z - 1),
    ];
    neighbours.sort_by_key(|(stand_x, stand_z)| {
        let side = match eff_angle {
            0 | 2 => *stand_x == door_x,
            _ => *stand_z == door_z,
        };
        let wanted = entry == 0 || (entry == 1 && side) || (entry == 2 && !side);
        ((!wanted) as u8, (stand_x - bot.world.x).abs() + (stand_z - bot.world.z).abs())
    });
    for (stand_x, stand_z) in neighbours {
        if bot.world.level == door_level && (bot.world.x, bot.world.z) == (stand_x, stand_z) {
            return true;
        }
        if bot.world.level == door_level
            && (bot.world.x - stand_x).abs() + (bot.world.z - stand_z).abs() == 1
            && lifescape_sdk::collision::step_open(&bot.collision, bot.world.x, bot.world.z, bot.world.level, stand_x - bot.world.x, stand_z - bot.world.z)
            && bot.step_to(stand_x, stand_z, 8).unwrap_or(false)
        {
            return true;
        }
    }
    // Already cardinally adjacent (or on the door): click from here.
    if bot.world.level == door_level && (bot.world.x - door_x).abs() + (bot.world.z - door_z).abs() <= 1 {
        return true;
    }
    false
}

/// Nearest known maze walllow door within `radius` tiles, if any.
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
    let Some((loc_id, shape, angle)) = bot.doors.remove(&(door_x, door_z, door_level)) else {
        return Err(Error::Unreachable(format!("no known door at ({door_x},{door_z},{door_level})")));
    };
    if maze_entry_side(loc_id).is_none() {
        bot.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        return Err(Error::Unreachable(format!("loc {loc_id} at ({door_x},{door_z}) is not a maze door")));
    }
    let Some(op) = bot.maps.as_ref().and_then(|pack| pack.loc_def(loc_id as usize)).and_then(|def| def.open_op) else {
        bot.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        return Ok(false);
    };
    if !approach_maze_door(bot, door_x, door_z, door_level, loc_id, angle, 800) {
        println!("{} maze door {loc_id} at ({door_x},{door_z}) unreachable from {:?}", bot.tag(), bot.position());
        bot.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        return Ok(false);
    }
    println!("{} opening maze door loc {loc_id} at ({door_x},{door_z},{door_level})", bot.tag());
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
    println!("{} maze door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut", bot.tag());
    if bot.world.modal.as_ref().is_some_and(|modal| modal.layer != "overlay") {
        let _ = bot.close_modal();
        let _ = bot.pump(2);
    }
    Ok(false)
}

/// Walk explicit steps one tile at a time inside the maze, swinging shut
/// maze doors that block the way. No router, no fan: diagonal steps split
/// into cardinals (open edge first), a shut edge swings a maze door when
/// there is one (ordinary shut doors go through the generic opener), and
/// anything else stops at once so the caller replans instead of
/// detouring blindly. True when every step is done (a teleport mid-route
/// stops early for the same reason).
pub fn walk_maze_steps(bot: &mut Bot, steps: &[(i32, i32)], timeout: u64) -> Result<bool> {
    let deadline = bot.world.ticks + timeout.max(1);
    bot.sync_live_doors();
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
        let (dx, dz) = (step_x - bot.world.x, step_z - bot.world.z);
        // Cardinal legs for this step, open edge first.
        let mut legs: Vec<(i32, i32)> = Vec::new();
        if dx != 0 && dz != 0 {
            let x_open = lifescape_sdk::collision::step_open(&bot.collision, bot.world.x, bot.world.z, bot.world.level, dx.signum(), 0);
            let z_open = lifescape_sdk::collision::step_open(&bot.collision, bot.world.x, bot.world.z, bot.world.level, 0, dz.signum());
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
            // Corner shut on both edges: swing a maze door there if
            // there is one, then replan from wherever that leaves us.
            for (tile_x, tile_z) in [(bot.world.x + dx.signum(), bot.world.z), (bot.world.x, bot.world.z + dz.signum())] {
                if let Some((loc_id, _, _)) = bot.doors.get(&(tile_x, tile_z, bot.world.level)).copied() {
                    if is_maze_door(loc_id) {
                        if let Err(error) = open_maze_door(bot, tile_x, tile_z, bot.world.level) {
                            if lifescape_sdk::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) {
                                return Err(error);
                            }
                        }
                        break;
                    }
                }
            }
            return Ok(false);
        }
        let mut advanced = false;
        for (move_x, move_z) in legs {
            if (bot.world.x, bot.world.z) == (step_x, step_z) {
                break;
            }
            let (next_x, next_z) = (bot.world.x + move_x, bot.world.z + move_z);
            eprintln!("{} maze walk leg ({},{})->({},{}) flags={:#x}", bot.tag(), bot.world.x, bot.world.z, next_x, next_z, bot.collision.get(next_x, next_z, bot.world.level));
            if !lifescape_sdk::collision::step_open(&bot.collision, bot.world.x, bot.world.z, bot.world.level, move_x, move_z) {
                eprintln!("{} maze walk leg blocked, door?", bot.tag());
                if let Some((loc_id, _, _)) = bot.doors.get(&(next_x, next_z, bot.world.level)).copied() {
                    if is_maze_door(loc_id) {
                        if let Err(error) = open_maze_door(bot, next_x, next_z, bot.world.level) {
                            if lifescape_sdk::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) {
                                return Err(error);
                            }
                        }
                        break;
                    }
                    if !bot.open_shut_door(next_x, next_z, bot.world.level, loc_id) {
                        break;
                    }
                } else {
                    break;
                }
            }
            match bot.step_to(next_x, next_z, 8) {
                Ok(true) => {
                    advanced = true;
                }
                Ok(false) => {
                    eprintln!("{} maze walk leg refused", bot.tag());
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
    use lifescape_sdk::collision::{self, MapPack};

    /// Pack index with no squares: offline stand-in so hand-painted
    /// collision survives untouched.
    fn empty_pack() -> MapPack {
        MapPack::new(std::path::PathBuf::from("no-pack.zip"), std::collections::HashMap::new(), Vec::new())
    }

    /// Stand choice follows the live angle, not the static paint: a door
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

    /// Maze door without an open action: the record survives the early
    /// exit, locked for every opener path.
    #[test]
    fn maze_door_without_open_op_keeps_registry() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        bot.doors.insert((10, 0, 0), (3628, 0, 2));
        assert!(!open_maze_door(&mut bot, 10, 0, 0).expect("no socket touched"), "no open action fails loud");
        assert_eq!(bot.doors.get(&(10, 0, 0)), Some(&(3628, 0, 2)), "record kept");
        assert_eq!(bot.world.ticks, 0, "failure pumps nothing");
    }

    /// Blocked maze legs stop at once: the solver replans from the new
    /// tile instead of detouring blindly. Offline the send never leaves,
    /// which reads as a typed login abort (never a silent Ok(false)).
    #[test]
    fn maze_steps_stop_on_shut_edge_without_door() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        collision::change_loc_collision(&mut bot.collision, 0, 2, true, 1, 1, 1, 0, 1, 0, true);
        let err = walk_maze_steps(&mut bot, &[(0, 1)], 60).expect_err("dead socket aborts typed");
        assert!(matches!(err, Error::Login(_)), "typed conn error, got: {err}");
        assert_eq!((bot.world.x, bot.world.z), (0, 0), "never moved");
    }
}
