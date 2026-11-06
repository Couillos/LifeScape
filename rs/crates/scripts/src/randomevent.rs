//! Random-event structure: where events happen, what they need.
//!
//! Detects the event (maze/mime/cube/
//! tower zones and the host/hostile casts). Solving stays in the rescue
//! scripts; this module only names the event and its rescue.

use lifescape_sdk::Bot;

/// Random event that must clear before a script can work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Maze,
    Mime,
    Cube,
    Tower,
    /// A visible event host (mage, genie, dwarf): greeted, never zoned.
    Host,
}

/// Unresolved event -> rescue script (short key without prefix).
/// Single mapping: the short key is the long name without its prefix.
pub fn rescue_for(event: EventKind) -> &'static str {
    match event {
        EventKind::Maze => "rescue_maze",
        EventKind::Tower => "rescue_down",
        EventKind::Cube => "rescue_cube",
        EventKind::Mime => "rescue_mime",
        EventKind::Host => "rescue_greet",
    }
}

/// Short rescue key for `rescues::run_rescue` (same mapping, no prefix).
pub fn rescue_key(event: EventKind) -> &'static str {
    rescue_for(event).strip_prefix("rescue_").unwrap_or("fallback")
}

/// Human event name for log lines (`event maze resolved=false`). The
/// worker owns the structured push; scripts never print control markers.
fn event_name(event: EventKind) -> &'static str {
    match event {
        EventKind::Maze => "maze",
        EventKind::Mime => "mime",
        EventKind::Cube => "cube",
        EventKind::Tower => "tower",
        EventKind::Host => "host",
    }
}

pub const EVENT_HOSTS: [&str; 3] = ["macro_mage", "macro_geni", "macro_dwarf"];
pub const HOSTILE_EVENTS: [&str; 2] = ["macro_swarm", "macro_triffidseed_angry"];

pub const MAZE_RECT: (i32, i32, i32, i32) = (2880, 2944, 4544, 4608);
pub const MIME_RECT: (i32, i32, i32, i32) = (1984, 2048, 4736, 4800);

fn in_rect(x: i32, z: i32, rect: (i32, i32, i32, i32)) -> bool {
    x >= rect.0 && x < rect.1 && z >= rect.2 && z < rect.3
}

pub fn in_maze(bot: &Bot) -> bool {
    in_maze_at(bot.world.x, bot.world.z)
}

/// Same check from a bare position (worker observer reads snapshots).
pub fn in_maze_at(x: i32, z: i32) -> bool {
    in_rect(x, z, MAZE_RECT)
}

pub fn in_mime(bot: &Bot) -> bool {
    in_mime_at(bot.world.x, bot.world.z)
}

/// Same check from a bare position (worker observer reads snapshots).
pub fn in_mime_at(x: i32, z: i32) -> bool {
    in_rect(x, z, MIME_RECT)
}

/// Fail-teleport spots (macro_event_fail_teleports_free): standing on one
/// of these upstairs proves an ignored host yanked the bot here.
pub const FAIL_TELEPORTS: [(i32, i32, i32); 25] = [
    (1, 3278, 3500), (1, 3097, 3268), (2, 2954, 3388), (3, 3221, 3474),
    (1, 3229, 3223), (0, 3097, 9567), (2, 3287, 3190), (1, 2964, 3213),
    (0, 2968, 3335), (0, 2935, 3515), (0, 3212, 3423), (0, 3076, 3412),
    (0, 2996, 3145), (1, 3025, 3251), (1, 2975, 3382), (1, 3214, 3215),
    (1, 3112, 3157), (0, 3246, 3451), (0, 3283, 3218), (0, 3045, 3491),
    (0, 3083, 3469), (2, 3104, 3163), (0, 3218, 9618), (0, 3263, 3424),
    // Last row keeps the 5x5 shape of the source table.
    (0, 0, 0),
];

pub fn in_fail_teleport(bot: &Bot) -> bool {
    in_fail_teleport_at(bot.position())
}

/// Same check from a bare position (worker observer reads snapshots).
pub fn in_fail_teleport_at(position: (i32, i32, i32)) -> bool {
    FAIL_TELEPORTS.contains(&position)
}

/// True when a strange box waits in the inventory.
pub fn cube_in_pack(bot: &Bot) -> bool {
    let Some(cube) = lifescape_gamedata::objs_by_name("macro_cube").into_iter().next() else {
        return false;
    };
    bot.inventory().get(&(cube.obj_id as u16)).copied().unwrap_or(0) > 0
}

/// Random event blocking work right now: maze, mime or a carried cube.
// Zone checks that need walking live in the rescue scripts.
pub fn event_at(bot: &Bot) -> Option<EventKind> {
    if in_maze(bot) {
        Some(EventKind::Maze)
    } else if in_mime(bot) {
        Some(EventKind::Mime)
    } else if cube_in_pack(bot) {
        Some(EventKind::Cube)
    } else {
        None
    }
}

/// Human event line for script logs (`event maze resolved=false`). The
/// worker poses the rescue and pushes the structured frame; scripts only
/// log.
pub fn event_line(bot: &Bot, event: EventKind, resolved: bool) -> String {
    event_line_at(bot.position(), event, resolved)
}

/// Same line from a bare position (worker observer reads snapshots).
pub fn event_line_at(position: (i32, i32, i32), event: EventKind, resolved: bool) -> String {
    let (level, x, z) = position;
    format!("event {} resolved={} at [{level},{x},{z}]", event_name(event), if resolved { "true" } else { "false" })
}

#[cfg(test)]
mod tests {
    use super::{EventKind, FAIL_TELEPORTS, rescue_for};

    #[test]
    fn rescues_match_legacy_map() {
        assert_eq!(rescue_for(EventKind::Maze), "rescue_maze");
        assert_eq!(rescue_for(EventKind::Tower), "rescue_down");
        assert_eq!(rescue_for(EventKind::Cube), "rescue_cube");
        assert_eq!(rescue_for(EventKind::Mime), "rescue_mime");
        assert_eq!(rescue_for(EventKind::Host), "rescue_greet");
        assert!(FAIL_TELEPORTS.contains(&(1, 3278, 3500)));
    }

    #[test]
    fn short_keys_match_run_rescue() {
        assert_eq!(super::rescue_key(EventKind::Maze), "maze");
        assert_eq!(super::rescue_key(EventKind::Tower), "down");
        assert_eq!(super::rescue_key(EventKind::Cube), "cube");
        assert_eq!(super::rescue_key(EventKind::Mime), "mime");
        assert_eq!(super::rescue_key(EventKind::Host), "greet");
        for kind in [EventKind::Maze, EventKind::Tower, EventKind::Cube, EventKind::Mime, EventKind::Host] {
            let long = super::rescue_for(kind);
            assert_eq!(long.strip_prefix("rescue_"), Some(super::rescue_key(kind)));
        }
    }

    #[test]
    fn event_line_carries_name_and_spot() {
        let line = super::event_line_at((0, 3200, 3200), EventKind::Maze, false);
        assert!(line.contains("maze"), "{line}");
        assert!(line.contains("resolved=false"), "{line}");
        assert!(line.contains("[0,3200,3200]"), "{line}");
        assert!(!line.contains("@@EVENT"), "{line}");
    }
}
