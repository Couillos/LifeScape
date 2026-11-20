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
//!
//! Teleport doors in use (2026-11-20, Mining Guild door 2112 (3046,9756),
//! 80 `door loc 2112 open but crossing … failed` a day, class C): the
//! content proc `~open_and_close_door` (`open_and_close_doors.rs2`)
//! teleports the clicker through, then `loc_change(inviswall, 3)` on the
//! door tile. Another miner's crossing read as a swing: the executor sent
//! a step into the invisible wall, which also cancelled its own queued
//! click, and the bot never moved. An inviswall on the door tile is a
//! door in use, never an open one: wait it out without stepping, then
//! click again (the click sent meanwhile found no door to open).

/// Invisible walls the content swaps onto a door tile mid-traverse
/// (`pack/loc.pack`: 83 `inviswall`, 2639 `inviswall_no_blockrange`; both
/// block walking).
pub(crate) const DOOR_IN_USE_LOCS: [u16; 2] = [83, 2639];

/// What the swing watch reads after one open click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoorWatch {
    /// On `cross_to`: done.
    Crossed,
    /// Moved onto the door tile by the click: the teleport model.
    Teleported,
    /// An invisible wall holds the door tile (someone's traverse in
    /// flight): never step, wait for it to clear.
    InUse,
    /// The live loc swung: step through now.
    Swung,
    /// Nothing yet.
    Wait,
}

/// Pure verdict of one watch tick: `at` the bot's tile, `door` and
/// `cross_to` the plan's, `clicked_from_door` true when the bot already
/// stood on the door tile at the click, `live_open` the swing reading,
/// `in_use` an invisible wall read on the door tile.
pub(crate) fn door_watch(at: (i32, i32, i32), door: (i32, i32, i32), cross_to: (i32, i32, i32), clicked_from_door: bool, live_open: bool, in_use: bool) -> DoorWatch {
    if at == cross_to {
        return DoorWatch::Crossed;
    }
    if at == door && !clicked_from_door {
        return DoorWatch::Teleported;
    }
    if in_use {
        return DoorWatch::InUse;
    }
    if live_open {
        return DoorWatch::Swung;
    }
    DoorWatch::Wait
}

impl crate::client::Bot {
    /// True when an invisible wall holds the door tile: a teleport door
    /// someone is crossing (`DOOR_IN_USE_LOCS`).
    pub(crate) fn door_in_use(&self, door: &lifescape_nav::plan::DoorRef) -> bool {
        self.world.loc_at(door.x, door.z, door.level).is_some_and(|(id, _, _)| DOOR_IN_USE_LOCS.contains(&id))
    }

    /// Waits, never stepping, until the door tile is no longer an
    /// invisible wall or `until` (server ticks) passes. True once free.
    pub(crate) fn wait_door_free(&mut self, door: &lifescape_nav::plan::DoorRef, until: u64) -> crate::error::Result<bool> {
        while self.door_in_use(door) {
            if self.world.ticks >= until {
                return Ok(false);
            }
            self.pump(1)?;
        }
        Ok(true)
    }
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
        assert_eq!(door_watch(DOOR, DOOR, CROSS, true, true, false), DoorWatch::Swung);
        assert_eq!(door_watch(DOOR, DOOR, CROSS, true, false, false), DoorWatch::Wait, "no swing yet: keep watching");
    }

    /// The teleport model stands when the click moved the bot onto the
    /// tile (manor backdoor), and a landing on cross_to always ends.
    #[test]
    fn a_click_that_moves_the_bot_onto_the_tile_is_the_teleport_model() {
        assert_eq!(door_watch(DOOR, DOOR, CROSS, false, false, false), DoorWatch::Teleported);
        assert_eq!(door_watch(CROSS, DOOR, CROSS, true, false, false), DoorWatch::Crossed);
        assert_eq!(door_watch((3107, 3272, 0), DOOR, CROSS, false, true, false), DoorWatch::Swung);
    }

    /// Zezima-11876 / will_field-47791: on the stand north of the guild
    /// door, the door tile turned into an inviswall one tick after the
    /// click (another miner crossing): wait, never a step into it. Our
    /// own crossing still ends the watch first.
    #[test]
    fn an_invisible_wall_on_the_door_is_a_door_in_use() {
        let guild_door = (3046, 9756, 0);
        let north_stand = (3046, 9757, 0);
        assert_eq!(door_watch(north_stand, guild_door, guild_door, false, true, true), DoorWatch::InUse);
        assert_eq!(door_watch(guild_door, guild_door, north_stand, true, true, true), DoorWatch::InUse, "clicked from the tile: still in use");
        assert_eq!(door_watch(guild_door, guild_door, guild_door, false, true, true), DoorWatch::Crossed);
    }
}
