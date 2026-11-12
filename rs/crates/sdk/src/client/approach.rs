//! Go near things: approach radius, single steps, step lists.
//!
//! The router (`movement`) plans the trek; this module executes the last
//! mile — walk within a radius, step one tile, replay step lists — and the
//! read-only `debug_walk` diagnosis. Splitting rule of the repo: one role
//! per module, no file above 1000 lines.

use crate::client::Bot;
use crate::client::movement::STEP_CAP;
use crate::error::{Error, Result};

impl Bot {
    pub fn approach(&mut self, x: i32, z: i32, level: i32, radius: i32, timeout: u64) -> Result<()> {
        let outermost = self.approach_depth == 0;
        if outermost {
            self.approach_opened.clear();
        }
        self.approach_depth += 1;
        let outcome = self.approach_guarded(x, z, level, radius, timeout);
        self.approach_depth -= 1;
        if outermost {
            self.approach_opened.clear();
        }
        outcome
    }

    fn approach_guarded(&mut self, x: i32, z: i32, level: i32, radius: i32, timeout: u64) -> Result<()> {
        self.act(format!("approach ({x},{z},{level}) r={radius} timeout={timeout}"));
        if self.world.level == level && (self.world.x - x).abs() + (self.world.z - z).abs() <= radius {
            return Ok(());
        }
        let dist = (self.world.x - x).abs() + (self.world.z - z).abs();
        let exact_budget = timeout.min(dist as u64 * 10 + 60);
        let mut cause = String::new();
        if let Err(err) = self.walk_to(x, z, level, exact_budget) {
            if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                return Err(err);
            }
            cause = format!(" exact: {err}");
        }
        if self.world.level == level && (self.world.x - x).abs() + (self.world.z - z).abs() <= radius {
            return Ok(());
        }
        let deadline = self.world.ticks + timeout;
        let here = (self.world.x, self.world.z);
        let mut candidates = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                candidates.push((x + dx, z + dz));
            }
        }
        candidates.sort_by_key(|tile| (tile.0 - here.0).abs() + (tile.1 - here.1).abs());
        let mut best = (self.world.x - x).abs() + (self.world.z - z).abs();
        let mut stagnant = 0u32;
        for (cx, cz) in candidates {
            // Pace: instant walk failures consume no ticks, so an unpaced
            // fan machine-guns the server without ever hitting a timeout.
            // Errors never fail here; the walks below surface real trouble.
            // A dead socket still aborts at once instead of pacing dead tiles.
            if let Err(err) = self.pump(1) {
                if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                    return Err(err);
                }
            }
            if (cx, cz) == here && self.world.level == level {
                return Ok(());
            }
            let rest = deadline.saturating_sub(self.world.ticks).max(250);
            match self.walk_to(cx, cz, level, rest) {
                Err(err) if crate::error::is_interrupted(&err) => return Err(err),
                // Dead socket, not a detour: surface it typed so the
                // session reconnects instead of pacing 169 dead tiles.
                Err(err) if matches!(err, Error::Io(_) | Error::Login(_)) => return Err(err),
                Err(err) if cause.is_empty() => {
                    cause = format!(" via ({cx},{cz}): {err}");
                }
                Ok(())
                    if self.world.level == level
                        && (self.world.x - x).abs() + (self.world.z - z).abs() <= radius =>
                {
                    return Ok(());
                }
                _ => {}
            }
            let here_dist = (self.world.x - x).abs() + (self.world.z - z).abs();
            if self.world.level == level && here_dist < best {
                best = here_dist;
                stagnant = 0;
            } else {
                stagnant += 1;
                if stagnant >= 10 {
                    return Err(Error::Unreachable(format!(
                        "no progress toward {x},{z} from {:?} after {stagnant} candidates without improvement.{cause}",
                        self.position()
                    )));
                }
            }
        }
        Err(Error::Unreachable(format!("nothing reachable within {radius} of {x},{z} from {:?}.{cause}", self.position())))
    }

    /// True when the bot stands where the server accepts a click on this
    /// NPC: same level, touching its footprint (`npc_size`, SW anchored)
    /// by a side. False when the slot left view.
    pub fn adjacent_to_npc(&self, slot: u16) -> bool {
        self.world.npcs.get(&slot).is_some_and(|npc| {
            let size = i32::from((self.tables.npc_size)(npc.npc_type).max(1));
            npc.level == self.world.level && side_adjacent((self.world.x, self.world.z), (npc.x, npc.z), size)
        })
    }

    /// Stand on a tile touching an NPC's footprint by a side: the server
    /// only accepts NPC clicks from a side, never from a corner. A size-2
    /// cow has two such tiles per side, not just the SW anchor's four.
    pub fn approach_npc(&mut self, slot: u16, timeout: u64) -> Result<()> {
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            let Some(npc) = self.world.npcs.get(&slot) else {
                return Err(Error::Unreachable(format!("NPC {slot} left view at {:?}", self.position())));
            };
            if npc.level != self.world.level {
                return Err(Error::Unreachable(format!("NPC {slot} is on level {} at {:?}", npc.level, self.position())));
            }
            if self.adjacent_to_npc(slot) {
                return Ok(());
            }
            let size = (self.tables.npc_size)(npc.npc_type).max(1) as i32;
            let mut ring = Vec::new();
            for dz in 0..size {
                ring.push((npc.x - 1, npc.z + dz));
                ring.push((npc.x + size, npc.z + dz));
            }
            for dx in 0..size {
                ring.push((npc.x + dx, npc.z - 1));
                ring.push((npc.x + dx, npc.z + size));
            }
            ring.sort_by_key(|tile| (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs());
            let mut moved = false;
            for (cx, cz) in ring {
                if self.world.ticks >= deadline {
                    break;
                }
                if (cx, cz) == (self.world.x, self.world.z) {
                    continue;
                }
                let rest = deadline.saturating_sub(self.world.ticks).max(250);
                // A walk to a blocked ring tile settles where we stand
                // with Ok: only real motion counts, or the same tile wins
                // every round and the loop below never sees a tick. A dead
                // socket still aborts typed instead of pacing the ring.
                let before = (self.world.x, self.world.z);
                match self.walk_to(cx, cz, self.world.level, rest) {
                    Err(err) if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) => return Err(err),
                    Ok(()) if (self.world.x, self.world.z) != before => {
                        moved = true;
                        break;
                    }
                    _ => {}
                }
            }
            if !moved {
                self.pump(1)?;
            }
            if self.adjacent_to_npc(slot) {
                return Ok(());
            }
        }
        Err(Error::Unreachable(format!("no clickable tile for NPC {slot} from {:?}", self.position())))
    }

    /// Walk one adjacent tile, without routing. True when the server moved us.
    pub fn step_to(&mut self, x: i32, z: i32, timeout: u64) -> Result<bool> {
        if (self.world.x, self.world.z) == (x, z) {
            return Ok(true);
        }
        if (x - self.world.x).abs() + (z - self.world.z).abs() != 1 {
            return Err(Error::Unreachable(format!("step_to is one tile, not {:?} -> {x},{z}", self.position())));
        }
        let run = self.want_run();
        self.send_walk(&[(x, z)], run, false)?;
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            self.pump(1)?;
            if (self.world.x, self.world.z) == (x, z) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Walk an explicit tile route, no router involved. True at the last tile.
    pub fn walk_steps(&mut self, steps: &[(i32, i32)], timeout: u64) -> Result<bool> {
        if steps.is_empty() {
            return Ok(true);
        }
        let run = self.want_run();
        for chunk in steps.chunks(STEP_CAP) {
            self.send_walk(chunk, run, false)?;
            let deadline = self.world.ticks + timeout.max(1);
            let target = chunk[chunk.len() - 1];
            while self.world.ticks < deadline {
                self.pump(1)?;
                if (self.world.x, self.world.z) == target {
                    break;
                }
            }
        }
        Ok((self.world.x, self.world.z) == steps[steps.len() - 1])
    }
}

/// Side adjacency to a square footprint anchored at its SW tile: on
/// the row just below/above it within its columns, or on the column
/// just left/right of it within its rows. Corners and tiles under the
/// footprint never count.
pub(crate) fn side_adjacent(tile: (i32, i32), anchor: (i32, i32), size: i32) -> bool {
    let size = size.max(1);
    let within_columns = tile.0 >= anchor.0 && tile.0 < anchor.0 + size;
    let within_rows = tile.1 >= anchor.1 && tile.1 < anchor.1 + size;
    (within_columns && (tile.1 == anchor.1 - 1 || tile.1 == anchor.1 + size))
        || (within_rows && (tile.0 == anchor.0 - 1 || tile.0 == anchor.0 + size))
}

#[cfg(test)]
mod tests {
    use crate::client::Bot;
    use crate::collision::{self, MapPack};

    /// Pack index with no squares, mirror of the movement-tests fixture:
    /// offline stand-in so hand-painted collision survives untouched.
    fn empty_pack() -> MapPack {
        MapPack::new(std::path::PathBuf::from("no-pack.zip"), std::collections::HashMap::new(), Vec::new())
    }

    /// `approach` on a blocked target from inside the radius: Ok without a
    /// single step (radius short-circuit, untouched by the fix).
    #[test]
    fn approach_blocked_target_within_radius_returns_ok() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 3216;
        bot.world.z = 3241;
        for tile_x in 3214..=3220 {
            for tile_z in 3238..=3244 {
                bot.collision.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        bot.collision.set(3217, 3241, 0, collision::WALK_BLOCKED);
        assert!(bot.approach(3217, 3241, 0, 2, 600).is_ok());
    }

    /// Blocked ring tile settling where we stand must not read as motion:
    /// before the fix the same tile won every round and no tick ever
    /// passed (worker wedged, session EAGAIN). Terminates offline.
    #[test]
    fn approach_npc_blocked_ring_tile_terminates() {
        use crate::world::NpcInfo;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        bot.collision.set(0, 0, 0, collision::OPEN);
        for tile in [(1, 0), (2, 0), (3, 0), (0, 1), (1, 1), (2, 1), (0, -1), (1, -1), (2, -1)] {
            bot.collision.set(tile.0, tile.1, 0, collision::WALK_BLOCKED);
        }
        bot.world.npcs.insert(7, NpcInfo { npc_type: 3, x: 2, z: 0, level: 0, name: None, anim: None });
        // Every ring walk settles back where we stand: motionless Ok must
        // not count, or this never reaches the pump below, offline or live.
        assert!(bot.approach_npc(7, 60).is_err(), "blocked ring terminates loud");
        assert_eq!((bot.world.x, bot.world.z), (0, 0), "never moved");
    }

    /// A size-2 cow at SW (3198,3331) covers (3198..3199, 3331..3332):
    /// both north-side tiles touch it. Live (bb4v5d9m 17:52:38) the
    /// size-1 check read them as "not adjacent" and the bot paced
    /// (3198,3333) <-> (3199,3333) until the budget ran out. Offline:
    /// any walk would fail, so Ok proves no step was taken.
    #[test]
    fn approach_npc_size_two_side_tile_is_adjacent() {
        use crate::tables::{Tables, NOTABLES};
        use crate::world::NpcInfo;
        fn size_two(_npc_type: u16) -> u8 {
            2
        }
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".").with_tables(Tables { npc_size: size_two, ..NOTABLES });
        bot.world.level = 0;
        bot.world.npcs.insert(472, NpcInfo { npc_type: 81, x: 3198, z: 3331, level: 0, name: None, anim: None });
        for tile in [(3198, 3333), (3199, 3333), (3200, 3331), (3197, 3332), (3199, 3330)] {
            bot.world.x = tile.0;
            bot.world.z = tile.1;
            assert!(bot.adjacent_to_npc(472), "{tile:?} touches the footprint by a side");
            assert!(bot.approach_npc(472, 60).is_ok(), "{tile:?} needs no step");
        }
        for tile in [(3197, 3330), (3200, 3333), (3198, 3331), (3199, 3332), (3201, 3331)] {
            bot.world.x = tile.0;
            bot.world.z = tile.1;
            assert!(!bot.adjacent_to_npc(472), "{tile:?} is a corner, under, or out of reach");
        }
    }

    /// Far target with no maps: the exact walk fails offline at the first
    /// send (never logged in), so the typed connection error aborts the fan
    /// before a single candidate is paced. Tick-bounded by construction
    /// (no pump ran), per-call memory cleared.
    #[test]
    fn approach_stops_without_progress() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let start_ticks = bot.world.ticks;
        let err = bot.approach(100, 100, 0, 6, 5000).unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("not logged in"), "typed conn error, got: {message}");
        assert_eq!(bot.world.ticks, start_ticks, "no pump ran offline");
        assert!(bot.approach_opened.is_empty(), "per-call memory cleared");
        assert_eq!(bot.approach_depth, 0, "depth restored");
    }
}
