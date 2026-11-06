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

    /// Stand on an orthogonally adjacent tile of an NPC: the server only
    /// accepts NPC clicks from a side, never from a diagonal.
    pub fn approach_npc(&mut self, slot: u16, timeout: u64) -> Result<()> {
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            let Some(npc) = self.world.npcs.get(&slot) else {
                return Err(Error::Unreachable(format!("NPC {slot} left view at {:?}", self.position())));
            };
            if npc.level != self.world.level {
                return Err(Error::Unreachable(format!("NPC {slot} is on level {} at {:?}", npc.level, self.position())));
            }
            if (npc.x - self.world.x).abs() + (npc.z - self.world.z).abs() == 1 {
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
            if let Some(npc) = self.world.npcs.get(&slot) {
                if npc.level == self.world.level && (npc.x - self.world.x).abs() + (npc.z - self.world.z).abs() == 1 {
                    return Ok(());
                }
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
        self.send_walk(&[(x, z)], run)?;
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
            self.send_walk(chunk, run)?;
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

    /// Walk explicit steps one tile at a time, swinging shut maze doors
    /// that block the way. No router, no fan: diagonal steps split into
    /// cardinals (open edge first), a shut edge swings a maze door when
    /// there is one, and anything else stops at once so the caller
    /// replans instead of detouring blindly. True when every step is
    /// done (a teleport mid-route stops early for the same reason).
    pub fn walk_path(&mut self, steps: &[(i32, i32)], timeout: u64) -> Result<bool> {
        let deadline = self.world.ticks + timeout.max(1);
        self.sync_live_doors();
        let mut index = 0;
        while index < steps.len() {
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!(
                    "walk_path timed out at {:?} ({} ticks)",
                    self.position(),
                    timeout
                )));
            }
            let (step_x, step_z) = steps[index];
            if (self.world.x, self.world.z) == (step_x, step_z) {
                index += 1;
                continue;
            }
            let (dx, dz) = (step_x - self.world.x, step_z - self.world.z);
            // Cardinal legs for this step, open edge first.
            let mut legs: Vec<(i32, i32)> = Vec::new();
            if dx != 0 && dz != 0 {
                let x_open = crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, dx.signum(), 0);
                let z_open = crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, 0, dz.signum());
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
                for (tile_x, tile_z) in [(self.world.x + dx.signum(), self.world.z), (self.world.x, self.world.z + dz.signum())] {
                    if let Some((loc_id, _, _)) = self.doors.get(&(tile_x, tile_z, self.world.level)).copied() {
                        if Self::is_maze_door(loc_id) {
                            if let Err(error) = self.open_maze_door(tile_x, tile_z, self.world.level) {
                                if crate::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) {
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
                if (self.world.x, self.world.z) == (step_x, step_z) {
                    break;
                }
                let (next_x, next_z) = (self.world.x + move_x, self.world.z + move_z);
                eprintln!("{} maze walk leg ({},{})->({},{}) flags={:#x}", self.tag(), self.world.x, self.world.z, next_x, next_z, self.collision.get(next_x, next_z, self.world.level));
                if !crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, move_x, move_z) {
                    eprintln!("{} maze walk leg blocked, door?", self.tag());
                    if let Some((loc_id, _, _)) = self.doors.get(&(next_x, next_z, self.world.level)).copied() {
                        if Self::is_maze_door(loc_id) {
                            if let Err(error) = self.open_maze_door(next_x, next_z, self.world.level) {
                                if crate::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) {
                                    return Err(error);
                                }
                            }
                            break;
                        }
                        if !self.open_shut_door(next_x, next_z, self.world.level, loc_id) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                match self.step_to(next_x, next_z, 8) {
                    Ok(true) => {
                        advanced = true;
                    }
                    Ok(false) => {
                        eprintln!("{} maze walk leg refused", self.tag());
                        break;
                    }
                    Err(error) if crate::error::is_interrupted(&error) || matches!(error, Error::Io(_) | Error::Login(_)) => return Err(error),
                    Err(_) => break,
                }
            }
            if (self.world.x, self.world.z) == (step_x, step_z) {
                index += 1;
            } else if !advanced {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
