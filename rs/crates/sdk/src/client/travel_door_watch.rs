//! Door swing watch after an open click (2026-11-17, gate 1551 of the
//! Draynor wheat field, 75 `door did not open … left us at (3107,3273),
//! want (3106,3273)` dossiers, jim_ellis/luke_hill picking wheat).
//!
//! A step-through on a live-open reading that turned stale leaves the
//! bot ON the door tile; the click path then opened the gate, but the
//! watch took "on the door tile" for the teleport model (`doors.rs2`
//! parks the clicker on the tile) before reading the swing, clicked the
//! gone closed id again (a no-op) and failed — the replan crossed the
//! open gate without a click. Being on the door tile is a teleport only
//! when the click moved the bot there.

/// What the swing watch reads after one open click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoorWatch {
    /// On `cross_to`: done.
    Crossed,
    /// Moved onto the door tile by the click: the teleport model.
    Teleported,
    /// The live loc swung: step through now.
    Swung,
    /// Nothing yet.
    Wait,
}

/// Pure verdict of one watch tick: `at` the bot's tile, `door` and
/// `cross_to` the plan's, `clicked_from_door` true when the bot already
/// stood on the door tile at the click, `live_open` the swing reading.
pub(crate) fn door_watch(at: (i32, i32, i32), door: (i32, i32, i32), cross_to: (i32, i32, i32), clicked_from_door: bool, live_open: bool) -> DoorWatch {
    if at == cross_to {
        return DoorWatch::Crossed;
    }
    if at == door && !clicked_from_door {
        return DoorWatch::Teleported;
    }
    if live_open {
        return DoorWatch::Swung;
    }
    DoorWatch::Wait
}

#[cfg(test)]
mod tests {
    use super::{door_watch, DoorWatch};

    const DOOR: (i32, i32, i32) = (3107, 3273, 0);
    const CROSS: (i32, i32, i32) = (3106, 3273, 0);

    /// jim_ellis 16:06:48: on the gate tile before the click, the gate
    /// swung: step through, never the teleport model's second click.
    #[test]
    fn a_click_from_the_door_tile_steps_through_the_swing() {
        assert_eq!(door_watch(DOOR, DOOR, CROSS, true, true), DoorWatch::Swung);
        assert_eq!(door_watch(DOOR, DOOR, CROSS, true, false), DoorWatch::Wait, "no swing yet: keep watching");
    }

    /// The teleport model stands when the click moved the bot onto the
    /// tile (manor backdoor), and a landing on cross_to always ends.
    #[test]
    fn a_click_that_moves_the_bot_onto_the_tile_is_the_teleport_model() {
        assert_eq!(door_watch(DOOR, DOOR, CROSS, false, false), DoorWatch::Teleported);
        assert_eq!(door_watch(CROSS, DOOR, CROSS, true, false), DoorWatch::Crossed);
        assert_eq!(door_watch((3107, 3272, 0), DOOR, CROSS, false, true), DoorWatch::Swung);
    }
}
