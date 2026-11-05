//! Random-event structure: where events happen, what they need.
//!
//! Mirrors `scripts/randomevents.py` event detection (maze/mime/cube/
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
}

/// Unresolved event -> rescue script (same mapping as the Python RESCUES).
pub fn rescue_for(event: EventKind) -> &'static str {
    match event {
        EventKind::Maze => "rescue_maze",
        EventKind::Tower => "rescue_down",
        EventKind::Cube => "rescue_cube",
        EventKind::Mime => "rescue_mime",
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
    in_rect(bot.world.x, bot.world.z, MAZE_RECT)
}

pub fn in_mime(bot: &Bot) -> bool {
    in_rect(bot.world.x, bot.world.z, MIME_RECT)
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
    FAIL_TELEPORTS.contains(&bot.position())
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

/// Orchestrator event payload (`@@EVENT` line), hand-built JSON.
pub fn report(bot: &Bot, event: EventKind, resolved: bool) -> String {
    let (level, x, z) = bot.position();
    format!(
        "@@EVENT {{\"kind\":\"random_event\",\"event\":\"{}\",\"resolved\":{},\"at\":[{level},{x},{z}]}}",
        match event {
            EventKind::Maze => "maze",
            EventKind::Mime => "mime",
            EventKind::Cube => "cube",
            EventKind::Tower => "tower",
        },
        if resolved { "true" } else { "false" }
    )
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
        assert!(FAIL_TELEPORTS.contains(&(1, 3278, 3500)));
    }
}
