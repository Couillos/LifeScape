//! Mill brick: the windmill clicks (windmills.rs2, 274). Hopper and bin
//! state are per player (perm varps `%hopper_full`, `%flour`); only the
//! lever (`hoppercontrol_inuse` for 3 ticks after any pull) and the bin
//! (`millbase`/`millbase_flour`) locs are shared. OPLOC carries no path:
//! every click is sent from a side stand of the loc, never the diagonal
//! the router settles on beside a blocked loc (live: every click from
//! (3165,3306) at Lumbridge answered "I can't reach that!").

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::survival::is_walk_abort;
use crate::skill_walk::walk_checked;
use crate::ScriptError;

/// Manhattan reach of the lever scan around the hopper.
const LEVER_RADIUS: i32 = 8;
/// Ticks a mill click has to answer (hopper fill and lever both run a
/// `p_arrivedelay`, windmills.rs2) before it reads as silent.
const CLICK_TICKS: u64 = 8;
/// Ticks to wait out a busy lever (`loc_change(hoppercontrol_inuse, 3)`
/// on every pull by any player, so a crowded mill flips it often).
const LEVER_BUSY_TICKS: u64 = 40;
/// Pulls tried while each click lands silent (lever flipped in between).
const LEVER_TRIES: u32 = 4;
/// Walk budget to one stand tile beside a mill loc.
const STAND_WALK_TICKS: u64 = 60;

pub fn loc_debugname(loc: u16) -> String {
    lifescape_gamedata::loc_by_id(usize::from(loc)).and_then(|entry| entry.name).unwrap_or_default().to_lowercase()
}

/// Nearest loc to `(x, z)` on `level` within `radius` whose debugname
/// passes `accept`: the live stream first (a `loc_change` such as
/// `millbase_flour` overrides the map), then one static batch from the
/// nav service. The worker paints no map, so `world.loc_at` alone never
/// saw the hopper (live 2026-11-14: every `mill_grain` run failed "no
/// hopper loc at (3166,3307)" on the hopper floor and stranded the bot
/// there).
pub fn station_loc(bot: &mut Bot, x: i32, z: i32, level: i32, radius: i32, accept: &dyn Fn(&str) -> bool) -> Result<Option<(i32, i32, u16)>, ScriptError> {
    let mut best: Option<(i32, (i32, i32, u16))> = None;
    let consider = |tile_x: i32, tile_z: i32, loc: u16, best: &mut Option<(i32, (i32, i32, u16))>| {
        let dist = (tile_x - x).abs() + (tile_z - z).abs();
        if dist <= radius && accept(&loc_debugname(loc)) && best.is_none_or(|(known, _)| dist < known) {
            *best = Some((dist, (tile_x, tile_z, loc)));
        }
    };
    for ((tile_x, tile_z, tile_level), slot) in &bot.world.locs {
        if let Some((loc, _, _)) = slot.as_ref().filter(|_| *tile_level == level) {
            consider(*tile_x, *tile_z, *loc, &mut best);
        }
    }
    if best.is_none() {
        for row in bot.static_locs_near(x, z, level, radius)? {
            // A tile the live stream changed or cleared is not the map's.
            if row.level == level && !bot.world.locs.contains_key(&(row.x, row.z, row.level)) {
                consider(row.x, row.z, row.loc, &mut best);
            }
        }
    }
    Ok(best.map(|(_, tile)| tile))
}

/// Operable lever near the hopper (`hopperlevers1-4`, op1 Operate), live
/// stream then map. `hoppercontrol_inuse` has no op1: OpLocHandler drops
/// a click on it silently, so it is never handed out — while the stream
/// shows it on the lever tile, that tile is not the map's and this reads
/// None (`lever_busy_near` tells busy from absent).
pub fn lever_near(bot: &mut Bot, x: i32, z: i32, level: i32) -> Result<Option<(i32, i32, u16)>, ScriptError> {
    station_loc(bot, x, z, level, LEVER_RADIUS, &|name| name.starts_with("hopperlevers"))
}

/// True while the stream shows `hoppercontrol_inuse` near the hopper (3
/// ticks after any player's pull, a global loc_change in windmills.rs2).
pub fn lever_busy_near(bot: &Bot, x: i32, z: i32, level: i32) -> bool {
    bot.world.locs.iter().any(|((tile_x, tile_z, tile_level), slot)| {
        *tile_level == level
            && (tile_x - x).abs() + (tile_z - z).abs() <= LEVER_RADIUS
            && slot.as_ref().is_some_and(|(loc, _, _)| loc_debugname(*loc) == "hoppercontrol_inuse")
    })
}

/// Server lines of the mill (windmills.rs2) and of the engine refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MillLine {
    /// "I can't reach that!" (Player.processInteraction): wrong stand.
    Unreachable,
    /// Grain put in, or the hopper already holds some (%hopper_full).
    HopperLoaded,
    /// Lever pulled on a full hopper: grain down the chute, %flour + 1.
    Milled,
    /// Lever pulled on an empty hopper.
    HopperEmpty,
    /// The bin holds 30 (%flour): empty it downstairs first.
    BinFull,
    /// A pot filled from the bin.
    FlourTaken,
    /// The bin is empty (%flour = 0).
    BinEmpty,
    /// No empty pot in the pack.
    NoPot,
}

/// Classify one game message as a mill line.
pub fn mill_line(message: &str) -> Option<MillLine> {
    let lowered = message.to_lowercase();
    let line = if lowered.contains("can't reach that") {
        MillLine::Unreachable
    } else if lowered.contains("put the grain in the hopper") || lowered.contains("already grain in the hopper") {
        MillLine::HopperLoaded
    } else if lowered.contains("grain slides down the chute") {
        MillLine::Milled
    } else if lowered.contains("operate the empty hopper") {
        MillLine::HopperEmpty
    } else if lowered.contains("flour bin downstairs is now full") || lowered.contains("flour bin downstairs is full") {
        MillLine::BinFull
    } else if lowered.contains("you fill a pot with") {
        MillLine::FlourTaken
    } else if lowered.contains("flour bin is already empty") {
        MillLine::BinEmpty
    } else if lowered.contains("need an empty pot") {
        MillLine::NoPot
    } else {
        return None;
    };
    Some(line)
}

/// Click a mill loc from its side stands, nearest first: walk onto the
/// stand, click, read the mill lines for `CLICK_TICKS`. A refusal moves
/// to the next stand; a `wanted` line ends the click with every line
/// read; a silent click (busy lever, stale id) returns empty. Every
/// stand refused is a typed failure.
pub fn click_from_stands(bot: &mut Bot, loc_at: (i32, i32, u16), click: &mut dyn FnMut(&mut Bot) -> Result<(), ScriptError>, wanted: &[MillLine]) -> Result<Vec<MillLine>, ScriptError> {
    let (x, z, loc) = loc_at;
    let stands = crate::kit::produce::loc_side_stands(bot, x, z, loc);
    for (stand_x, stand_z) in stands.iter().copied() {
        match walk_checked(bot, stand_x, stand_z, STAND_WALK_TICKS) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) if is_walk_abort(&error) => return Err(error),
            Err(_) => continue,
        }
        // Blocked décor settles the router beside it: only the stand clicks.
        if (bot.world.x, bot.world.z) != (stand_x, stand_z) {
            continue;
        }
        let seen = bot.world.messages.len();
        click(bot)?;
        let deadline = bot.world.ticks + CLICK_TICKS;
        let mut refused = false;
        while bot.world.ticks < deadline {
            bot.pump(1)?;
            let lines: Vec<MillLine> = bot.world.messages.get(seen..).unwrap_or(&[]).iter().filter_map(|message| mill_line(message)).collect();
            if lines.iter().any(|line| wanted.contains(line)) {
                return Ok(lines);
            }
            if lines.contains(&MillLine::Unreachable) {
                refused = true;
                break;
            }
        }
        if !refused {
            return Ok(Vec::new());
        }
    }
    Err(ScriptError::Failed(format!("no stand reaches mill loc {} at ({x},{z}): tried {stands:?} from {:?}", loc_debugname(loc), bot.position())))
}

/// Pull the hopper lever once it is operable: waits out
/// `hoppercontrol_inuse`, re-reads the lever before each pull (its id
/// flips under other players) and retries a silent pull.
pub fn pull_lever(bot: &mut Bot, hopper: (i32, i32, i32)) -> Result<Vec<MillLine>, ScriptError> {
    let (hx, hz, hl) = hopper;
    for _ in 0..LEVER_TRIES {
        let deadline = bot.world.ticks + LEVER_BUSY_TICKS;
        let lever = loop {
            if let Some(lever) = lever_near(bot, hx, hz, hl)? {
                break Some(lever);
            }
            if !lever_busy_near(bot, hx, hz, hl) || bot.world.ticks >= deadline {
                break None;
            }
            bot.pump(1)?;
        };
        let Some((lever_x, lever_z, lever_id)) = lever else {
            let busy = lever_busy_near(bot, hx, hz, hl);
            return Err(ScriptError::Failed(format!("no operable hopper lever near ({hx},{hz}) (busy {busy}) at {:?}", bot.position())));
        };
        let lines = click_from_stands(bot, (lever_x, lever_z, lever_id), &mut |step_bot| Ok(step_bot.op_loc(1, lever_x, lever_z, lever_id)?), &[MillLine::Milled, MillLine::HopperEmpty, MillLine::BinFull])?;
        if !lines.is_empty() {
            return Ok(lines);
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::{lever_busy_near, lever_near, loc_debugname, mill_line, station_loc, MillLine};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    fn loc_named(name: &str) -> u16 {
        (0u16..u16::MAX).find(|id| loc_debugname(*id) == name).expect("loc in the dump")
    }

    /// Lever scan: a live `hopperlevers` loc near the hopper wins without
    /// asking the map.
    #[test]
    fn lever_resolves_live_first() {
        let mut bot = offline_bot();
        bot.world.level = 2;
        let lever = loc_named("hopperlevers1");
        bot.world.locs.insert((3167, 3307, 2), Some((lever, 0, 0)));
        assert_eq!(lever_near(&mut bot, 3166, 3307, 2).expect("live, no service needed"), Some((3167, 3307, lever)), "lever resolves");
    }

    /// Live 2026-11-14: the worker paints no map, so the hopper, its
    /// lever and the millbase (static locs, never streamed) were never
    /// found ("no hopper loc at (3166,3307)") and the bot stayed on the
    /// hopper floor. The map serves them: hopper_lumbridge at
    /// (3166,3307,2), hopperlevers1 at (3166,3305,2), millbase at
    /// (3166,3307,0) (m49_51.jm2); other levels and far tiles never match.
    #[test]
    fn mill_locs_resolve_from_the_map() {
        let (mut bot, _service) = crate::served_nav_bot("mill-locs");
        bot.world.level = 2;
        let hopper = station_loc(&mut bot, 3166, 3307, 2, 0, &|name| name == "hopper_lumbridge").expect("served");
        assert_eq!(hopper.map(|(x, z, loc)| (x, z, loc_debugname(loc))), Some((3166, 3307, "hopper_lumbridge".to_string())));
        let lever = lever_near(&mut bot, 3166, 3307, 2).expect("served").expect("the lever from the map");
        assert_eq!((lever.0, lever.1, loc_debugname(lever.2)), (3166, 3305, "hopperlevers1".to_string()));
        let base = station_loc(&mut bot, 3166, 3307, 0, 0, &|name| name.starts_with("millbase")).expect("served");
        assert_eq!(base.map(|(x, z, loc)| (x, z, loc_debugname(loc))), Some((3166, 3307, "millbase".to_string())));
        assert!(lever_near(&mut bot, 3166, 3307, 0).expect("served").is_none(), "other levels never match");
        assert!(lever_near(&mut bot, 3000, 3000, 2).expect("served").is_none(), "far tiles never match");
    }

    /// `hoppercontrol_inuse` (3 ticks after any pull, global loc_change in
    /// windmills.rs2) has no op1: OpLocHandler drops the click silently.
    /// The scan never hands it out — not even the map's lever under it —
    /// it only reads as busy.
    #[test]
    fn busy_lever_is_never_clicked() {
        let (mut bot, _service) = crate::served_nav_bot("mill-busy");
        bot.world.level = 2;
        bot.world.locs.insert((3166, 3305, 2), Some((loc_named("hoppercontrol_inuse"), 10, 2)));
        assert_eq!(lever_near(&mut bot, 3166, 3307, 2).expect("served"), None, "the in-use lever is not operable");
        assert!(lever_busy_near(&bot, 3166, 3307, 2), "the in-use lever reads busy");
        bot.world.locs.insert((3166, 3305, 2), Some((loc_named("hopperlevers1"), 10, 2)));
        assert_eq!(lever_near(&mut bot, 3166, 3307, 2).expect("served"), Some((3166, 3305, loc_named("hopperlevers1"))));
        assert!(!lever_busy_near(&bot, 3166, 3307, 2), "a free lever is not busy");
    }

    /// OPLOC carries no path: the travel arrival (3165,3306) is diagonal to
    /// the Lumbridge hopper (2x1 at 3166,3307 level 2), its lever (1x1 at
    /// 3166,3305) and the flour bin (2x2 at 3166,3307 level 0) — live,
    /// every click there answered "I can't reach that!". Stands are side
    /// tiles only, nearest the bot first, angles from the served map.
    #[test]
    fn mill_stands_are_sides_never_the_diagonal_arrival() {
        let (mut bot, _service) = crate::served_nav_bot("mill-stands");
        bot.world.level = 2;
        bot.world.x = 3165;
        bot.world.z = 3306;
        let hopper_stands = crate::kit::produce::loc_side_stands(&mut bot, 3166, 3307, loc_named("hopper_lumbridge"));
        assert_eq!(hopper_stands.first(), Some(&(3165, 3307)), "west of the hopper first, got {hopper_stands:?}");
        assert!(hopper_stands.contains(&(3168, 3307)) && hopper_stands.contains(&(3167, 3308)), "east and north sides, got {hopper_stands:?}");
        let lever_stands = crate::kit::produce::loc_side_stands(&mut bot, 3166, 3305, loc_named("hopperlevers1"));
        assert!(lever_stands.contains(&(3165, 3305)) && lever_stands.contains(&(3167, 3305)), "lever sides, got {lever_stands:?}");
        bot.world.level = 0;
        let bin_stands = crate::kit::produce::loc_side_stands(&mut bot, 3166, 3307, loc_named("millbase"));
        assert_eq!(&bin_stands[..2], &[(3165, 3307), (3166, 3306)], "bin sides nearest first, got {bin_stands:?}");
        for stands in [&hopper_stands, &lever_stands, &bin_stands] {
            assert!(!stands.contains(&(3165, 3306)), "the diagonal arrival never stands, got {stands:?}");
        }
    }

    /// The windmills.rs2 lines the mill loop reads.
    #[test]
    fn mill_lines_read_the_server_script() {
        let cases = [
            ("I can't reach that!", MillLine::Unreachable),
            ("You put the grain in the hopper.", MillLine::HopperLoaded),
            ("There is already grain in the hopper.", MillLine::HopperLoaded),
            ("You operate the hopper. The grain slides down the chute.", MillLine::Milled),
            ("You operate the empty hopper. Nothing interesting happens.", MillLine::HopperEmpty),
            ("The flour bin downstairs is now full.", MillLine::BinFull),
            ("The flour bin downstairs is full, I should empty it first.", MillLine::BinFull),
            ("You fill a pot with flour from the bin.", MillLine::FlourTaken),
            ("You fill a pot with the last of the flour in the bin.", MillLine::FlourTaken),
            ("The flour bin is already empty.", MillLine::BinEmpty),
            ("You need an empty pot to hold the flour in.", MillLine::NoPot),
        ];
        for (line, expected) in cases {
            assert_eq!(mill_line(line), Some(expected), "{line}");
        }
        assert_eq!(mill_line("Welcome to RuneScape."), None);
    }
}
