//! Maze random-event solver: state-space search over walking plus door
//! teleports, then stepwise execution with verification.
//!
//! The maze layout is fixed (content map m45_71, confirmed against the
//! wiki: same layout, 4 spawns). Doors do not open passages to walk
//! through: clicking one teleports the player (open_and_close_door in
//! `macro_event_maze.rs2` and door_procs.rs2). Planning "through" doors
//! with the generic router therefore ping-pongs, so this module searches
//! the true state space instead: walk steps on open floor plus directed
//! door edges (stand -> teleport destination).

use std::collections::{HashMap, HashSet, VecDeque};

use lifescape_sdk::Bot;
use lifescape_sdk::collision::LOC;

use super::quest_common as kit;
use super::randomevent;
use super::maze_doors;
use crate::{ScriptError, ScriptResult};

/// check_axis (door_procs.rs2): W/E doors read the same-X tiles, N/S
/// doors the same-Z ones.
fn check_axis(angle: u8, player_x: i32, player_z: i32, door_x: i32, door_z: i32) -> bool {
    match angle {
        0 | 2 => player_x == door_x,
        _ => player_z == door_z,
    }
}

/// Through-destination of a swing (door_open for straight walls, the only
/// shape the maze uses).
fn door_dest(angle: u8, door_x: i32, door_z: i32) -> (i32, i32) {
    match angle {
        0 => (door_x - 1, door_z),
        1 => (door_x, door_z + 1),
        2 => (door_x + 1, door_z),
        _ => (door_x, door_z - 1),
    }
}

/// Walk step allowed: same shared edge test as the SDK walker (both
/// wall sides plus walkable tiles), so planned steps execute as-is.
fn maze_step_open(collision: &lifescape_sdk::collision::CollisionMap, x: i32, z: i32, dx: i32, dz: i32, level: i32) -> bool {
    lifescape_sdk::collision::step_open(collision, x, z, level, dx, dz)
}

/// True when Touch can land: cardinal-adjacent to the statue footprint
/// with no wall on the shared edge, or standing inside it. Diagonal
/// corners refuse ("can't reach that").
fn statue_touchable(bot: &Bot, statue_x: i32, statue_z: i32, width: i32, length: i32) -> bool {
    let (bot_x, bot_z) = (bot.world.x, bot.world.z);
    let (far_x, far_z) = (statue_x + width - 1, statue_z + length - 1);
    let near = bot_x >= statue_x - 1 && bot_x <= far_x + 1 && bot_z >= statue_z && bot_z <= far_z + 1
        || bot_x >= statue_x && bot_x <= far_x && bot_z >= statue_z - 1 && bot_z <= far_z + 1;
    if !near {
        return false;
    }
    let card_or_inside =
        (bot_x >= statue_x && bot_x <= far_x) || (bot_z >= statue_z && bot_z <= far_z);
    if !card_or_inside {
        return false;
    }
    // Click tile facing the bot (getLoc matches origin tiles only, so the
    // click always goes to the origin; the edge must be open towards it).
    // Standing inside needs no edge.
    let click_x = bot_x.clamp(statue_x, far_x);
    let click_z = bot_z.clamp(statue_z, far_z);
    let (step_x, step_z) = (click_x - bot_x, click_z - bot_z);
    if step_x == 0 && step_z == 0 {
        return true;
    }
    let level = bot.world.level;
    let here = bot.collision.get(bot_x, bot_z, level);
    let there = bot.collision.get(click_x, click_z, level);
    use lifescape_sdk::collision::{WALL_EAST, WALL_NORTH, WALL_SOUTH, WALL_WEST};
    match (step_x, step_z) {
        (0, 1) => here & WALL_NORTH == 0 && there & WALL_SOUTH == 0,
        (0, -1) => here & WALL_SOUTH == 0 && there & WALL_NORTH == 0,
        (1, 0) => here & WALL_EAST == 0 && there & WALL_WEST == 0,
        (-1, 0) => here & WALL_WEST == 0 && there & WALL_EAST == 0,
        _ => false,
    }
}

/// Click the statue origin (getLoc matches origin tiles only) and wait
/// out. True when the maze let go.
fn touch_statue(bot: &mut Bot, statue_x: i32, statue_z: i32, statue_loc: u16) -> Result<bool, ScriptError> {
    bot.op_loc(1, statue_x, statue_z, statue_loc)?;
    // Exit runs anim + p_delay(5): gone within ~7 ticks, cap at 10.
    let target = bot.world.ticks + 10;
    while bot.world.ticks < target && randomevent::in_maze(bot) {
        bot.pump(1)?;
    }
    Ok(!randomevent::in_maze(bot))
}

/// One planned action: walk explicit steps, swing a door, touch the
/// statue.
enum MazeAction {
    Walk(Vec<(i32, i32)>),
    OpenDoor(i32, i32, i32),
    Touch,
}

/// Shortest action list from the bot to touching the statue: BFS over
/// tiles, where walk edges cost 1 and door swings (stand -> teleport
/// destination, entry side permitting) cost 2 clicks. Doors in `tried`
/// already refused us and read shut for planning.
fn plan(
    bot: &mut Bot,
    statue_x: i32,
    statue_z: i32,
    statue_w: i32,
    statue_l: i32,
    tried: &HashSet<(i32, i32, i32)>,
) -> Option<Vec<MazeAction>> {
    let level = bot.world.level;
    let (statue_far_x, statue_far_z) = (statue_x + statue_w - 1, statue_z + statue_l - 1);
    // Collect maze doors on this level with their entry sides, except
    // ones that already refused us (they read shut until relogin).
    let mut doors: Vec<(i32, i32, u16, u8, u8)> = Vec::new();
    for ((tile_x, tile_z, tile_level), (loc_id, shape, angle)) in bot.doors.iter() {
        if *tile_level != level {
            continue;
        }
        if tried.contains(&(*tile_x, *tile_z, *tile_level)) {
            continue;
        }
        if maze_doors::maze_entry_side(*loc_id).is_some() {
            doors.push((*tile_x, *tile_z, *loc_id, *shape, *angle));
        }
    }
    // BFS state: tile -> (parent tile, action to get here).
    // Door swings land directly on their destination tile.
    #[derive(Clone, Copy)]
    enum Edge {
        Walk,
        Door(i32, i32, i32),
    }
    let start = (bot.world.x, bot.world.z);
    let mut parent: HashMap<(i32, i32), ((i32, i32), Edge)> = HashMap::new();
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    let mut queue = VecDeque::new();
    seen.insert(start);
    queue.push_back(start);
    bot.log(&format!("maze plan: start={:?} doors={} statue=({},{})", start, doors.len(), statue_x, statue_z));
    let mut goal: Option<(i32, i32)> = None;
    let mut expanded = 0usize;
    // Bounds: the maze square plus a margin (BFS stays inside anyway).
    let in_bounds = |(x, z): (i32, i32)| x >= 2880 && x < 2944 && z >= 4544 && z < 4608;
    // Walkable mirrors the SDK router: a tile holds unless every
    // direction is blocked. LOC alone does not hold (doors sit on wall
    // tiles), but scenery footprints (statue, chests) are never walked.
    let walkable = |bot_ref: &Bot, tile: (i32, i32)| {
        let flags = bot_ref.collision.get(tile.0, tile.1, level);
        let in_statue = tile.0 >= statue_x
            && tile.0 <= statue_far_x
            && tile.1 >= statue_z
            && tile.1 <= statue_far_z;
        !in_statue
            && lifescape_sdk::collision::tile_walkable(flags)
            && flags & LOC == 0
    };
    while let Some(current) = queue.pop_front() {
        expanded += 1;
        // Goal: touchable from here (checks edges + inside).
        // Reuse the live check via a lightweight probe: same geometry as
        // statue_touchable but on the candidate tile.
        let (cx, cz) = current;
        let near = cx >= statue_x - 1 && cx <= statue_far_x + 1 && cz >= statue_z && cz <= statue_far_z + 1
            || cx >= statue_x && cx <= statue_far_x && cz >= statue_z - 1 && cz <= statue_far_z + 1;
        let card_or_inside =
            (cx >= statue_x && cx <= statue_far_x) || (cz >= statue_z && cz <= statue_far_z);
        if near && card_or_inside {
            // Edge check like statue_touchable (walls only; statue LOC is fine).
            use lifescape_sdk::collision::{WALL_EAST, WALL_NORTH, WALL_SOUTH, WALL_WEST};
            let click_x = cx.clamp(statue_x, statue_far_x);
            let click_z = cz.clamp(statue_z, statue_far_z);
            let open = match (click_x - cx, click_z - cz) {
                (0, 0) => true,
                (0, 1) => {
                    let here = bot.collision.get(cx, cz, level);
                    let there = bot.collision.get(click_x, click_z, level);
                    here & WALL_NORTH == 0 && there & WALL_SOUTH == 0
                }
                (0, -1) => {
                    let here = bot.collision.get(cx, cz, level);
                    let there = bot.collision.get(click_x, click_z, level);
                    here & WALL_SOUTH == 0 && there & WALL_NORTH == 0
                }
                (1, 0) => {
                    let here = bot.collision.get(cx, cz, level);
                    let there = bot.collision.get(click_x, click_z, level);
                    here & WALL_EAST == 0 && there & WALL_WEST == 0
                }
                (-1, 0) => {
                    let here = bot.collision.get(cx, cz, level);
                    let there = bot.collision.get(click_x, click_z, level);
                    here & WALL_WEST == 0 && there & WALL_EAST == 0
                }
                _ => false,
            };
                if open {
                    goal = Some(current);
                    break;
                } else if near && card_or_inside {
                    bot.log(&format!("maze near-miss {:?} edge shut", current));
                }
        }
        // Walk edges.
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = (current.0 + dx, current.1 + dz);
            if !in_bounds(next) || seen.contains(&next) || !walkable(bot, next) {
                continue;
            }
            if !maze_step_open(&bot.collision, current.0, current.1, dx, dz, level) {
                continue;
            }
            seen.insert(next);
            parent.insert(next, (current, Edge::Walk));
            queue.push_back(next);
        }
        // Door edges: every cardinal stand of every known maze door.
        for (door_x, door_z, _loc_id, shape, angle) in &doors {
            if *shape != 0 {
                continue;
            }
            let entry = match maze_doors::maze_entry_side(*_loc_id) {
                Some(entry) => entry,
                None => continue,
            };
            for (stand_x, stand_z) in [
                (*door_x + 1, *door_z),
                (*door_x - 1, *door_z),
                (*door_x, *door_z + 1),
                (*door_x, *door_z - 1),
            ] {
                if (stand_x, stand_z) != current {
                    continue;
                }
                let side = check_axis(*angle, stand_x, stand_z, *door_x, *door_z);
                let allowed = entry == 0 || (entry == 1 && side) || (entry == 2 && !side);
                if !allowed {
                    continue;
                }
                // Entering from the true side (or from the door tile
                // itself) teleports through; a false-side click parks on
                // the door, whose next click goes through.
                let dest = if side || (stand_x, stand_z) == (*door_x, *door_z) {
                    door_dest(*angle, *door_x, *door_z)
                } else {
                    (*door_x, *door_z)
                };
                if !in_bounds(dest) || seen.contains(&dest) {
                    continue;
                }
                bot.log(&format!("maze door edge {:?} -> {:?} -> {:?}", current, (door_x, door_z), dest));
                seen.insert(dest);
                parent.insert(dest, (current, Edge::Door(*door_x, *door_z, level)));
                queue.push_back(dest);
            }
        }
    }
    if goal.is_none() {
        let (mut x0, mut x1, mut z0, mut z1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        let mut stands = 0usize;
        for (sx, sz) in seen.iter() {
            x0 = x0.min(*sx);
            x1 = x1.max(*sx);
            z0 = z0.min(*sz);
            z1 = z1.max(*sz);
        }
        for (door_x, door_z, _, _, _) in doors.iter() {
            for (stand_x, stand_z) in [(*door_x + 1, *door_z), (*door_x - 1, *door_z), (*door_x, *door_z + 1), (*door_x, *door_z - 1)] {
                if seen.contains(&(stand_x, stand_z)) {
                    stands += 1;
                }
            }
        }
        bot.log(&format!("maze plan: NO PATH expanded={} seen={} x=[{},{}] z=[{},{}] doorstands={} from {:?}", expanded, seen.len(), x0, x1, z0, z1, stands, start));
    }
    let goal = goal?;
    // Reconstruct tile path with edge kinds, then compress walk runs.
    let mut tiles: Vec<(i32, i32)> = vec![goal];
    let mut edges: Vec<Edge> = Vec::new();
    while tiles.last().copied() != Some(start) {
        let tile = *tiles.last().unwrap();
        let (prev, edge) = parent.get(&tile).copied()?;
        tiles.push(prev);
        edges.push(edge);
    }
    tiles.reverse();
    edges.reverse();
    let mut actions: Vec<MazeAction> = Vec::new();
    let mut run: Vec<(i32, i32)> = Vec::new();
    let flush = |run: &mut Vec<(i32, i32)>, actions: &mut Vec<MazeAction>| {
        if !run.is_empty() {
            actions.push(MazeAction::Walk(std::mem::take(run)));
        }
    };
    for (tile, edge) in tiles.into_iter().skip(1).zip(edges.into_iter()) {
        match edge {
            Edge::Walk => run.push(tile),
            Edge::Door(door_x, door_z, door_level) => {
                flush(&mut run, &mut actions);
                actions.push(MazeAction::OpenDoor(door_x, door_z, door_level));
            }
        }
    }
    flush(&mut run, &mut actions);
    actions.push(MazeAction::Touch);
    Some(actions)
}

/// Solve the maze: follow BFS door-to-door plans, touching as soon as
/// the statue is in reach. True when out.
pub fn solve(bot: &mut Bot) -> Result<bool, ScriptError> {
    if !randomevent::in_maze(bot) {
        return Ok(true);
    }
    bot.ensure_area(bot.world.x, bot.world.z, 70);
    bot.ensure_area(2911, 4575, 10);
    let Ok((statue_x, statue_z, _statue_level, statue_loc)) = kit::find_loc(bot, &["macro_maze_complete"], 150) else {
        bot.log("maze centre missing from the map table");
        return Ok(false);
    };
    let (statue_w, statue_l) = lifescape_gamedata::loc_by_id(statue_loc as usize)
        .map(|entry| (entry.width as i32, entry.length as i32))
        .unwrap_or((3, 3));
    let deadline = bot.world.ticks + 15000;
    let mut tried: HashSet<(i32, i32, i32)> = HashSet::new();
    let mut stuck = 0u32;
    while randomevent::in_maze(bot) && bot.world.ticks < deadline {
        // Real dialogs only: the maze timer is an overlay, permanent and
        // harmless, closing it every round just burns ticks.
        if bot.world.modal.as_ref().is_some_and(|modal| modal.layer != "overlay") {
            bot.close_modal()?;
            bot.pump(3)?;
        }
        if statue_touchable(bot, statue_x, statue_z, statue_w, statue_l) {
            if touch_statue(bot, statue_x, statue_z, statue_loc)? {
                return Ok(true);
            }
        }
        let before = (bot.world.x, bot.world.z);
        let Some(actions) = plan(bot, statue_x, statue_z, statue_w, statue_l, &tried) else {
            bot.log(&format!("maze: no planned path to the centre from {:?}", bot.position()));
            return Ok(false);
        };
        for action in actions {
            if !randomevent::in_maze(bot) {
                return Ok(true);
            }
            match action {
                MazeAction::Walk(steps) => {
                    // Explicit steps, no router: the BFS already routed
                    // through doors; the maze stepper swings shut ones met.
                    match maze_doors::walk_maze_steps(bot, &steps, 120) {
                        Ok(_) => {}
                        Err(error) if lifescape_sdk::error::is_interrupted(&error) => {
                            return Err(ScriptError::Sdk(error));
                        }
                        Err(_) => {}
                    }
                }
                MazeAction::OpenDoor(door_x, door_z, door_level) => {
                    // Live diagnostic before the click: exact loc, bot
                    // position, chosen side and why (entry, live-or-static
                    // angle, axis). Same wording as the SDK per-click line.
                    let (diag_loc, _diag_shape, diag_angle) =
                        bot.doors.get(&(door_x, door_z, door_level)).copied().unwrap_or((3628, 0, 0));
                    let diag_live = bot.world.loc_at(door_x, door_z, door_level);
                    let diag_entry = maze_doors::maze_entry_side(diag_loc).unwrap_or(0);
                    let (diag_angle_eff, diag_source) =
                        maze_doors::maze_effective_angle(diag_live, diag_loc, diag_angle);
                    bot.log(&maze_doors::maze_door_diag(
                        door_x, door_z, door_level, diag_live,
                        bot.world.x, bot.world.z, bot.world.level,
                        diag_entry, diag_angle_eff, diag_source, bot.world.x, bot.world.z,
                    ));
                    match maze_doors::open_maze_door(bot, door_x, door_z, door_level) {
                        Ok(true) => {}
                        Ok(false) => {
                            tried.insert((door_x, door_z, door_level));
                        }
                        Err(error) if lifescape_sdk::error::is_interrupted(&error) => {
                            return Err(ScriptError::Sdk(error));
                        }
                        Err(_) => {
                            tried.insert((door_x, door_z, door_level));
                        }
                    }
                    break;
                }
                MazeAction::Touch => {
                    if touch_statue(bot, statue_x, statue_z, statue_loc)? {
                        return Ok(true);
                    }
                }
            }
            if statue_touchable(bot, statue_x, statue_z, statue_w, statue_l) {
                break;
            }
        }
        // Yield every round: the failure paths above do no pump of their
        // own, and without one unload/call stay busy until the deadline.
        bot.pump(1)?;
        if (bot.world.x, bot.world.z) == before {
            stuck += 1;
            if stuck >= 3 {
                bot.log(&format!("maze: no progress from {:?}", bot.position()));
                return Ok(false);
            }
        } else {
            stuck = 0;
        }
    }
    Ok(!randomevent::in_maze(bot))
}

/// Thin wrapper so the rescue registry keeps one entry point.
pub fn run(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    if solve(bot)? { Ok(0) } else { Ok(1) }
}
