//! Fire lines: where a bot may light a row of logs, the server's rules
//! (`skill_firemaking/scripts/firemaking.rs2`, rev 274):
//!
//! - `~area_allow_loc_add(coord)`: refused ("You can't light a fire
//!   here.") when an interactable loc (`active=1`, any layer, its whole
//!   footprint) covers the tile (`map_locaddunsafe`: an existing fire
//!   too), when the tile is `map_blocked` (`WALK_BLOCKED`, members land
//!   on a free world) or inside a `bank_zones` box;
//! - `~push_player` after each fire: west if `lineofwalk` allows the one
//!   step, else east, south, north, else stay on the fire.
//!
//! So a load of N logs wants N legal tiles in a straight row going WEST
//! from the start, each west step walkable. The grid is pure (tests feed
//! it); `scan` fills it from the nav service plus live locs.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_gamedata as knowledge;
use lifescape_nav::collision::{BLOCK_WEST, WALK_BLOCKED};
use lifescape_sdk::Bot;

use crate::ScriptError;

/// Search radius (Chebyshev) for a line start around the anchor.
pub const START_RADIUS: i32 = 14;
/// Footprints bleed from origins this far south-west (largest F2P loc
/// widths): the static scan reaches that much past the box.
const FOOTPRINT_BLEED: i32 = 4;

/// One tile as the planner sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FireTile {
    /// Collision flags (server values).
    pub flags: u32,
    /// An active loc's footprint covers it (static or live).
    pub occupied: bool,
    /// Free-to-play land (members land reads blocked on a free world).
    pub f2p: bool,
}

/// Pure grid on one level: unknown tiles are illegal (never guessed).
#[derive(Debug, Clone, Default)]
pub struct FireGrid {
    pub level: i32,
    pub tiles: HashMap<(i32, i32), FireTile>,
}

impl FireGrid {
    /// Server `~area_allow_loc_add` on this tile.
    pub fn legal(&self, x: i32, z: i32) -> bool {
        self.tiles.get(&(x, z)).is_some_and(|tile| tile.f2p && !tile.occupied && tile.flags & WALK_BLOCKED == 0)
            && !knowledge::fire_zones::in_bank_zone(x, z, self.level)
    }

    /// Server `lineofwalk(coord, west)` for one step: the west tile
    /// takes no `WALL_EAST` and no walk block.
    pub fn west_step(&self, x: i32, z: i32) -> bool {
        self.tiles.get(&(x - 1, z)).is_some_and(|tile| tile.f2p && tile.flags & BLOCK_WEST == 0)
    }

    /// Legal fires in a row from `(x, z)` going west, up to `want`: each
    /// next tile is legal and reached by a west step.
    pub fn run_west(&self, x: i32, z: i32, want: u32) -> u32 {
        if !self.legal(x, z) {
            return 0;
        }
        let mut length = 1u32;
        let mut at = x;
        while length < want && self.west_step(at, z) && self.legal(at - 1, z) {
            at -= 1;
            length += 1;
        }
        length
    }

    /// Best start near `from`: a full row (`want` fires) nearest first;
    /// else the longest row of at least `min` (nearest on ties). Banned
    /// starts (refused live) are skipped. `(x, z, length)`.
    pub fn best_line(&self, from: (i32, i32), want: u32, min: u32, banned: &HashSet<(i32, i32)>) -> Option<(i32, i32, u32)> {
        let mut best: Option<((bool, u32, i32), (i32, i32, u32))> = None;
        for (&(x, z), _) in &self.tiles {
            if (x - from.0).abs().max((z - from.1).abs()) > START_RADIUS || banned.contains(&(x, z)) {
                continue;
            }
            let length = self.run_west(x, z, want);
            if length < min.min(want) {
                continue;
            }
            let distance = (x - from.0).abs() + (z - from.1).abs();
            // Lower is better: full rows first, then longer, then nearer.
            let key = (length < want, want - length, distance);
            if best.as_ref().is_none_or(|(known, (known_x, known_z, _))| (key, (x, z)) < (*known, (*known_x, *known_z))) {
                best = Some((key, (x, z, length)));
            }
        }
        best.map(|(_, line)| line)
    }
}

/// Tiles an active loc's footprint covers: width x length from the
/// origin, swapped for north/south angles (engine `map_locaddunsafe`).
pub fn footprint(origin: (i32, i32), loc_id: u16, angle: u8) -> Vec<(i32, i32)> {
    let Some(def) = knowledge::loc_by_id(usize::from(loc_id)) else { return Vec::new() };
    if def.active != 1 {
        return Vec::new();
    }
    let (width, length) = if angle == 1 || angle == 3 { (def.length, def.width) } else { (def.width, def.length) };
    let mut tiles = Vec::new();
    for delta_x in 0..width.max(1) as i32 {
        for delta_z in 0..length.max(1) as i32 {
            tiles.push((origin.0 + delta_x, origin.1 + delta_z));
        }
    }
    tiles
}

/// Fill a grid around `anchor` for rows of up to `want` fires: service
/// collision flags and F2P verdicts (two batches), static active locs
/// (one scan), then live locs on top (a fire lit by someone else, a loc
/// the server removed). A down service is a typed error, never a guess.
pub fn scan(bot: &mut Bot, anchor: (i32, i32), want: u32) -> Result<FireGrid, ScriptError> {
    let level = bot.world.level;
    let reach = want as i32;
    let (min_x, max_x) = (anchor.0 - START_RADIUS - reach, anchor.0 + START_RADIUS);
    let (min_z, max_z) = (anchor.1 - START_RADIUS, anchor.1 + START_RADIUS);
    let mut coords: Vec<(i32, i32, i32)> = Vec::new();
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            coords.push((x, z, level));
        }
    }
    let rows = bot.tiles_flags(&coords).map_err(ScriptError::Sdk)?;
    let f2p = bot.in_f2p_batch(&coords).map_err(ScriptError::Sdk)?;
    let mut grid = FireGrid { level, tiles: HashMap::new() };
    for (row, inside) in rows.iter().zip(f2p) {
        grid.tiles.insert((row.tile.x, row.tile.z), FireTile { flags: row.flags, occupied: false, f2p: inside });
    }
    let center = ((min_x + max_x) / 2, (min_z + max_z) / 2);
    let radius = (max_x - min_x) / 2 + (max_z - min_z) / 2 + 2 * FOOTPRINT_BLEED;
    let statics = bot.static_locs_near(center.0, center.1, level, radius).map_err(ScriptError::Sdk)?;
    let mut covered: HashSet<(i32, i32)> = HashSet::new();
    for placed in statics {
        // A live change at the origin wins over the template.
        if bot.world.locs.contains_key(&(placed.x, placed.z, level)) {
            continue;
        }
        covered.extend(footprint((placed.x, placed.z), placed.loc, placed.angle));
    }
    for (&(x, z, loc_level), live) in &bot.world.locs {
        if loc_level != level {
            continue;
        }
        if let Some((loc, _, angle)) = live {
            covered.extend(footprint((x, z), *loc, *angle));
        }
    }
    for tile in covered {
        if let Some(entry) = grid.tiles.get_mut(&tile) {
            entry.occupied = true;
        }
    }
    Ok(grid)
}

/// True when a live active loc (a fire someone lit) now covers the tile.
pub fn live_occupied(bot: &Bot, x: i32, z: i32) -> bool {
    bot.world.locs.iter().any(|(&(origin_x, origin_z, level), live)| {
        level == bot.world.level
            && live.is_some_and(|(loc, _, angle)| footprint((origin_x, origin_z), loc, angle).contains(&(x, z)))
    })
}

/// Shortest row worth walking to when no full row exists nearby.
pub const MIN_ROW: u32 = 4;
/// Fresh plans per load before failing loud (refusals, short rows,
/// fires lit by others in the way).
const MAX_PLANS: u32 = 12;
/// Lighting can take many 4-tick attempts at low level (stat_random
/// 64/512): wait this long for the catch before failing the click.
const CATCH_TICKS: u64 = 200;
/// Server lines (`firemaking.rs2`).
const CATCH_LINE: &str = "the fire catches";
const REFUSED_LINE: &str = "can't light a fire here";

/// One light attempt's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightOutcome {
    /// Lit; `west` when the push moved the bot one tile west (the row
    /// goes on), false when it moved elsewhere or stayed.
    Lit { west: bool },
    /// "You can't light a fire here.": the tile is illegal live.
    Refused,
}

/// Burn every carried `log` in planned west rows; returns fires lit.
/// Each row starts on a legal tile chosen from static collision plus
/// live locs, every fire is verified live (catch line, push west); a
/// refusal bans that start and a broken row replans from where the bot
/// stands. Never lights where it merely stands (the Draynor bank).
pub fn burn_load(bot: &mut Bot, log: u16, tinderbox: u16) -> Result<u32, ScriptError> {
    let mut banned: HashSet<(i32, i32)> = HashSet::new();
    let mut lit = 0u32;
    let mut plans = 0u32;
    while crate::kit::pack::pack_total(bot, log) > 0 {
        plans += 1;
        if plans > MAX_PLANS {
            return Err(ScriptError::Failed(format!("no fire row held after {MAX_PLANS} plans near {:?}", bot.position())));
        }
        let want = crate::kit::pack::pack_total(bot, log);
        let here = (bot.world.x, bot.world.z);
        let grid = scan(bot, here, want)?;
        let Some((start_x, start_z, length)) = grid.best_line(here, want, MIN_ROW, &banned) else {
            return Err(ScriptError::Failed(format!("no legal fire row of {MIN_ROW}+ tiles within {START_RADIUS} of {:?}", bot.position())));
        };
        bot.log(&format!("fire row: {length} tiles west from ({start_x},{start_z}) for {want} logs"));
        bot.intend(&format!("lighting fires from ({start_x},{start_z})"));
        if !crate::skill_walk::walk_checked(bot, start_x, start_z, 1500)? || (bot.world.x, bot.world.z) != (start_x, start_z) {
            bot.log(&format!("fire row start ({start_x},{start_z}) not reached, replanning from {:?}", bot.position()));
            banned.insert((start_x, start_z));
            continue;
        }
        for _ in 0..length {
            if crate::kit::pack::pack_total(bot, log) == 0 {
                break;
            }
            let (at_x, at_z) = (bot.world.x, bot.world.z);
            if !grid.legal(at_x, at_z) || live_occupied(bot, at_x, at_z) {
                break;
            }
            match light_here(bot, log, tinderbox)? {
                LightOutcome::Lit { west } => {
                    lit += 1;
                    if !west {
                        break;
                    }
                }
                LightOutcome::Refused => {
                    bot.log(&format!("fire refused at ({at_x},{at_z}), banning the row start ({start_x},{start_z})"));
                    banned.insert((start_x, start_z));
                    banned.insert((at_x, at_z));
                    break;
                }
            }
        }
    }
    Ok(lit)
}

/// Light one log on the current tile (logs on the tinderbox, proven
/// order) and read the server's verdict: the refusal line, or the catch
/// line then the push (west first). No reply in time is a loud failure.
pub fn light_here(bot: &mut Bot, log: u16, tinderbox: u16) -> Result<LightOutcome, ScriptError> {
    let here = (bot.world.x, bot.world.z);
    let seen = bot.world.messages.len();
    let (com, slot) = crate::quests::common::inv_slot(bot, log)?;
    let (use_com, use_slot) = crate::quests::common::inv_slot(bot, tinderbox)?;
    bot.op_held_u(log, slot, com, tinderbox, use_slot, use_com)?;
    let deadline = bot.world.ticks + CATCH_TICKS;
    let mut caught_at: Option<u64> = None;
    while bot.world.ticks < deadline {
        bot.pump(2)?;
        let fresh: Vec<String> = bot.world.messages.iter().skip(seen).map(|line| line.to_lowercase()).collect();
        if fresh.iter().any(|line| line.contains(REFUSED_LINE)) {
            return Ok(LightOutcome::Refused);
        }
        if caught_at.is_none() && fresh.iter().any(|line| line.contains(CATCH_LINE)) {
            caught_at = Some(bot.world.ticks);
        }
        // The push lands the same server tick as the catch; a few ticks
        // without a move means boxed in (the bot stays on its fire).
        if caught_at.is_some_and(|at| (bot.world.x, bot.world.z) != here || bot.world.ticks >= at + 4) {
            break;
        }
    }
    if caught_at.is_none() {
        return Err(ScriptError::Failed(format!("log did not catch fire at {here:?} in {CATCH_TICKS} ticks")));
    }
    Ok(LightOutcome::Lit { west: (bot.world.x, bot.world.z) == (here.0 - 1, here.1) })
}

#[cfg(test)]
mod tests {
    use super::{FireGrid, FireTile};
    use lifescape_nav::collision::{LOC, WALL_EAST};
    use std::collections::HashSet;

    fn open_grid(level: i32, min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> FireGrid {
        let mut grid = FireGrid { level, ..FireGrid::default() };
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                grid.tiles.insert((x, z), FireTile { flags: 0, occupied: false, f2p: true });
            }
        }
        grid
    }

    /// Server rules on one tile: blocked, occupied, members land and the
    /// bank boxes refuse; a wall on the west tile's east edge stops the
    /// push west (the fire row ends there).
    #[test]
    fn legal_tiles_and_west_steps_follow_the_server() {
        let mut grid = open_grid(0, 3100, 3130, 3300, 3310);
        assert!(grid.legal(3110, 3305));
        grid.tiles.get_mut(&(3110, 3305)).expect("tile").flags = LOC;
        assert!(!grid.legal(3110, 3305), "blocked tile");
        grid.tiles.get_mut(&(3111, 3305)).expect("tile").occupied = true;
        assert!(!grid.legal(3111, 3305), "existing fire or active loc");
        grid.tiles.get_mut(&(3112, 3305)).expect("tile").f2p = false;
        assert!(!grid.legal(3112, 3305), "members land");
        assert!(!grid.legal(3140, 3305), "unknown tile never guessed");
        grid.tiles.get_mut(&(3119, 3306)).expect("tile").flags = WALL_EAST;
        assert!(!grid.west_step(3120, 3306), "wall east of the west tile");
        assert_eq!(grid.run_west(3125, 3306, 27), 6, "row stops at the wall: 3125..3120");
        let bank = open_grid(0, 3080, 3100, 3238, 3250);
        assert!(!bank.legal(3092, 3243), "Draynor bank box: 'You can't light a fire here.'");
        assert!(bank.legal(3092, 3248), "north of the bank");
    }

    /// The live failure: a bot standing in the Draynor bank lit in
    /// place. The planner never starts inside the bank, prefers a full
    /// 27-fire row, and skips refused starts.
    #[test]
    fn best_line_leaves_the_bank_and_wants_full_rows() {
        let mut grid = open_grid(0, 3050, 3110, 3230, 3256);
        // A tree row two tiles north cuts every row there short.
        for x in 3050..=3110 {
            grid.tiles.get_mut(&(x, 3250)).expect("tile").flags = LOC;
        }
        for x in (3050..=3110).step_by(5) {
            grid.tiles.get_mut(&(x, 3249)).expect("tile").occupied = true;
        }
        let banned = HashSet::new();
        let (x, z, length) = grid.best_line((3092, 3243), 27, 5, &banned).expect("a row exists");
        assert_eq!(length, 27, "full row for a full load");
        assert!(!lifescape_gamedata::fire_zones::in_bank_zone(x, z, 0), "start outside the bank: ({x},{z})");
        for step in 0..27 {
            assert!(grid.legal(x - step, z), "every fire tile legal: ({},{z})", x - step);
        }
        assert_ne!(z, 3250, "blocked row never picked");
        assert_ne!(z, 3249, "short rows lose to full ones");
        let mut refused = HashSet::new();
        refused.insert((x, z));
        let (other_x, other_z, _) = grid.best_line((3092, 3243), 27, 5, &refused).expect("another row");
        assert_ne!((other_x, other_z), (x, z), "a refused start is skipped");
    }

    /// Real map (nav service on the repo dumps): a bot banking at Draynor
    /// (inside the bank box, where the server said "You can't light a
    /// fire here.") gets a full 27-fire row outside, every tile free of
    /// blocks and active locs, every west step walkable.
    #[test]
    fn draynor_bank_plans_a_legal_row_outside() {
        let (mut bot, _service) = crate::served_nav_bot("fire-draynor");
        bot.world.x = 3092;
        bot.world.z = 3243;
        let grid = super::scan(&mut bot, (3092, 3243), 27).expect("service answers");
        assert!(!grid.legal(3092, 3243), "inside the bank is refused");
        let (x, z, length) = grid.best_line((3092, 3243), 27, super::MIN_ROW, &HashSet::new()).expect("a row near Draynor");
        assert_eq!(length, 27, "a full row exists near the bank, got {length} at ({x},{z})");
        for step in 0..27 {
            assert!(grid.legal(x - step, z), "fire tile ({},{z}) legal", x - step);
            assert!(!lifescape_gamedata::fire_zones::in_bank_zone(x - step, z, 0));
        }
        for step in 0..26 {
            assert!(grid.west_step(x - step, z), "push west from ({},{z})", x - step);
        }
    }
}
