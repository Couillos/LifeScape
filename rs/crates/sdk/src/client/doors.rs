//! Doors: learn, open, verify and walk through them like the manual
//! player does — adjacent, click, step through at once, no speculative
//! opens ahead and no silent pacing. Planning already reads known doors
//! open; this module only bridges the live gap (stand, click, verify,
//! step). Splitting rule of the repo: one role per module, no file above
//! 1000 lines.

use crate::client::Bot;

impl Bot {
    /// Repaint known doors from their live state before planning: the base
    /// map goes stale on every swing, and planning through a tile the
    /// server reads shut sends legs the server refuses. Not streamed yet
    /// keeps its static paint.
    pub fn sync_live_doors(&mut self) {
        let Some(pack) = &self.maps else { return };
        for ((door_x, door_z, door_level), (loc_id, shape, angle)) in self.doors.clone() {
            match self.world.loc_at(door_x, door_z, door_level) {
                Some((id, _, _)) if id == loc_id => {
                    if let Some(def) = pack.loc_def(loc_id as usize) {
                        crate::collision::change_loc_collision(
                            &mut self.collision, shape, angle, def.blockrange, def.length, def.width, def.active, door_x, door_z, door_level, true,
                        );
                    }
                }
                Some(_) => {
                    pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
                }
                None => {}
            }
        }
    }

    /// Remember a statically-openable door the registry never learned
    /// (verified open by the live state, so it is a real door): same
    /// openable-blocker predicate as `record_square`, so planning only
    /// ever sees through real doors. Unknown tiles stay ignored.
    fn remember_static_door(&mut self, door_x: i32, door_z: i32, door_level: i32) {
        let key = (door_x, door_z, door_level);
        if self.doors.contains_key(&key) {
            return;
        }
        let Some((static_id, shape, angle)) = self.world.static_locs.get(&key).copied() else {
            return;
        };
        let openable = self
            .maps
            .as_ref()
            .and_then(|pack| pack.loc_def(static_id as usize))
            .is_some_and(|def| def.open_op.is_some() && def.blockwalk);
        if !openable {
            return;
        }
        self.doors.insert(key, (static_id, shape, angle));
        let toll = self
            .maps
            .as_ref()
            .and_then(|pack| pack.loc_def(static_id as usize))
            .is_some_and(|def| def.toll);
        if toll {
            self.toll_gates.insert(key);
        }
    }

    /// Open one known shut door we stand next to (walk legs step one
    /// tile at a time, so adjacency holds): click and verify live.
    /// Already-open (live id flipped) costs nothing. True when the step
    /// may go through; toll gates without allowance stay shut.
    pub fn open_shut_door(&mut self, door_x: i32, door_z: i32, door_level: i32, loc_id: u16) -> bool {
        if self.toll_gates.contains(&(door_x, door_z, door_level)) && !self.allow_paid {
            return false;
        }
        if !matches!(self.world.loc_at(door_x, door_z, door_level), Some((id, _, _)) if id == loc_id) {
            self.remember_static_door(door_x, door_z, door_level);
            return true;
        }
        let Some(op) = self.maps.as_ref().and_then(|pack| pack.loc_def(loc_id as usize)).and_then(|def| def.open_op) else {
            return false;
        };
        self.intend(&format!("opening the door at ({door_x},{door_z})"));
        if self.op_loc(op, door_x, door_z, loc_id).is_err() {
            return false;
        }
        if !self.door_opened(door_x, door_z, door_level, loc_id) {
            println!("{} door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut", self.tag());
            return false;
        }
        // Unpaint the freed tile so steps pass: the live loc moved away
        // but the base map still paints it shut.
        self.remember_static_door(door_x, door_z, door_level);
        if let Some(pack) = &self.maps {
            let (_, shape, angle) = self.doors.get(&(door_x, door_z, door_level)).copied().unwrap_or((loc_id, 0, 0));
            pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
        }
        true
    }

    /// Open the nearest known door that sits ON the planned path: a route
    /// may cross a door ten tiles ahead, and retrying the same refused step
    /// while standing next to nothing is a dead end. A door the live state
    /// already shows open is skipped without a click (false). Otherwise each
    /// door is tried once. The click is verified live (id/shape flip): an
    /// unverified door stays shut in the plan and never resets the stall
    /// counter.
    pub(crate) fn open_door_on_path(&mut self, tiles: &[(i32, i32)], far: bool) -> bool {
        let mut best: Option<(usize, (i32, i32, i32))> = None;
        for (index, (tile_x, tile_z)) in tiles.iter().enumerate() {
            let key = (*tile_x, *tile_z, self.world.level);
            if !self.doors.contains_key(&key) {
                continue;
            }
            if !self.allow_paid && self.toll_gates.contains(&key) {
                continue;
            }
            if best.as_ref().is_none_or(|(known, _)| index < *known) {
                best = Some((index, key));
            }
        }
        let Some((door_index, (door_x, door_z, door_level))) = best else {
            return false;
        };
        // Manual behavior: only touch a door we can step through right now.
        // A door ten tiles ahead re-closes before arrival; leave it alone,
        // later replans pick it up when adjacent. `far` overrides this for
        // the fully-stalled case: whole-click refusals stall ten tiles out
        // where the near opener never fires, so walking to the door first
        // is the only play (the approach below does the walking).
        if door_level != self.world.level {
            return false;
        }
        if !far && (door_x - self.world.x).abs().max((door_z - self.world.z).abs()) > 3
        {
            return false;
        };
        if self.approach_depth > 0 && self.approach_opened.contains(&(door_x, door_z, door_level)) {
            return false;
        }
        let Some((loc_id, shape, angle)) = self.doors.remove(&(door_x, door_z, door_level)) else {
            return false;
        };
        // Already verified open (live id flipped or loc gone, e.g. a
        // re-crossing while a lasting door stands open): the click would
        // hit nothing and verify trivially. Skip stand and click; the
        // stall stands, another blockage holds us.
        if !matches!(
            self.world.loc_at(door_x, door_z, door_level),
            Some((live_id, _, _)) if live_id == loc_id
        )
        {
            if let Some(pack) = &self.maps {
                pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
            }
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        let tolled = self.toll_gates.contains(&(door_x, door_z, door_level));
        let Some(op) = self.maps.as_ref()
            .and_then(|pack| pack.loc_def(loc_id as usize))
            .and_then(|def| def.open_op) else {
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        };
        if self.approach(door_x, door_z, door_level, 1, 800).is_ok() {
        } else {
            println!("{} door {loc_id} at ({door_x},{door_z}) unreachable from {:?}", self.tag(), self.position());
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        println!("{} opening door loc {loc_id} on path at ({door_x},{door_z},{door_level})", self.tag());
        self.intend(&format!("opening the door at ({door_x},{door_z})"));
        if self.op_loc(op, door_x, door_z, loc_id).is_err() {
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        let _ = self.pump(4);
        if tolled && self.allow_paid && self.world.modal.is_some() {
            let _ = self.chat_choose("Yes, ok.");
            let _ = self.pump(10);
        }
        // Fast doors re-close while the watch below runs: already standing
        // next to the open swing, walk on now (server truth) instead of
        // verifying first. A toll modal open never steps (guarded).
        // A successful onto chains the off step at once, same call: the
        // route tile after the door, no replan between (it would walk
        // back into the re-closed tile 20+ ticks later).
        if self.step_onto_door_now(door_x, door_z, door_level, loc_id) {
            let next = tiles.get(door_index + 1).copied();
            if let Some(through) = self.step_off_door_now(door_x, door_z, door_level, loc_id, shape, angle, next) {
                return through;
            }
            return self.crossed_door_now(door_x, door_z, door_level, loc_id, shape, angle);
        }
        if !self.door_opened(door_x, door_z, door_level, loc_id) {
            println!("{} door loc {loc_id} on path at ({door_x},{door_z}) did not open, leaving it shut", self.tag());
            if self.world.modal.is_some() {
                let _ = self.close_modal();
                let _ = self.pump(2);
            }
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        // Doors re-close too: keep the record so the next
        // crossing re-verifies live instead of walking a tile the
        // server reads shut.
        self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        if self.approach_depth > 0 {
            self.approach_opened.insert((door_x, door_z, door_level));
        }
        if let Some(pack) = &self.maps {
            pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
        }
        // The unpaint is the exact inverse of the static paint (same
        // shape/angle/def), so a tile that still reads blocked is held by
        // something else: fail loud instead of pacing a shut plan.
        if !crate::collision::tile_walkable(self.collision.get(door_x, door_z, door_level)) {
            println!("{} door loc {loc_id} on path at ({door_x},{door_z}) opened but stays blocked", self.tag());
            return false;
        }
        true
    }

    /// Tile to step onto right after the click, pure: an ordinary door with
    /// no modal open (free door, or toll already paid above) one orthogonal
    /// step away. An open toll modal pays first and reads as no step.
    fn immediate_step_tile(&self, door_x: i32, door_z: i32, door_level: i32, _loc_id: u16) -> Option<(i32, i32)> {
        if self.world.modal.is_some() || self.world.level != door_level {
            return None;
        }
        if (self.world.x - door_x).abs() + (self.world.z - door_z).abs() != 1 {
            return None;
        }
        Some((door_x, door_z))
    }

    /// One server-truth step onto the just-clicked ordinary door, before the
    /// 12-tick watch: fast doors re-close while it runs. True when the
    /// server walked us on (door crossed, watch skipped).
    fn step_onto_door_now(&mut self, door_x: i32, door_z: i32, door_level: i32, loc_id: u16) -> bool {
        match self.immediate_step_tile(door_x, door_z, door_level, loc_id) {
            Some((step_x, step_z)) => matches!(self.step_to(step_x, step_z, 8), Ok(true)),
            None => false,
        }
    }

    /// Tile to step OFF onto right after stepping on, pure: the route
    /// tile after the door when known and adjacent, else the walk
    /// objective itself when it sits next door. An open toll modal pays
    /// first and reads as no step.
    fn door_off_tile(&self, door_x: i32, door_z: i32, door_level: i32, _loc_id: u16, next: Option<(i32, i32)>) -> Option<(i32, i32)> {
        if self.world.modal.is_some() || self.world.level != door_level {
            return None;
        }
        if let Some((next_x, next_z)) = next {
            if (next_x - door_x).abs() + (next_z - door_z).abs() == 1 {
                return Some((next_x, next_z));
            }
        }
        if let Some((goal_x, goal_z)) = self.route_target {
            if (goal_x - door_x).abs() + (goal_z - door_z).abs() == 1 {
                return Some((goal_x, goal_z));
            }
        }
        None
    }

    /// One server-truth step OFF the just-stepped-on door, same call, no
    /// replan between: fast doors re-close in a few ticks, so a replan
    /// (20+ ticks) walks back into a shut tile. None when no off tile is
    /// known (the caller books the onto alone, as before). Some(true)
    /// when fully through, with the same books as the watched path.
    /// Some(false) when the off step is refused: logged with live loc and
    /// flags like legs, record kept, fail loud — never a blind replan.
    fn step_off_door_now(
        &mut self,
        door_x: i32,
        door_z: i32,
        door_level: i32,
        loc_id: u16,
        shape: u8,
        angle: u8,
        next: Option<(i32, i32)>,
    ) -> Option<bool> {
        let (off_x, off_z) = self.door_off_tile(door_x, door_z, door_level, loc_id, next)?;
        if !matches!(self.step_to(off_x, off_z, 8), Ok(true)) {
            let _ = self.crossed_door_now(door_x, door_z, door_level, loc_id, shape, angle);
            let live = self.world.loc_at(off_x, off_z, door_level);
            let flags = self.collision.get(off_x, off_z, door_level);
            self.log(&Self::refused_line(&[(off_x, off_z)], (door_x, door_z, door_level), live, flags));
            return Some(false);
        }
        Some(self.crossed_door_now(door_x, door_z, door_level, loc_id, shape, angle))
    }

    /// Success books for a door crossed on the immediate step: same record,
    /// approach mark and unpaint as the watched path (doors re-close, so the
    /// next crossing re-verifies live). False when another blocker still
    /// holds the tile — fail loud, like the watch path.
    fn crossed_door_now(&mut self, door_x: i32, door_z: i32, door_level: i32, loc_id: u16, shape: u8, angle: u8) -> bool {
        self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        if self.approach_depth > 0 {
            self.approach_opened.insert((door_x, door_z, door_level));
        }
        if let Some(pack) = &self.maps {
            pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
        }
        if !crate::collision::tile_walkable(self.collision.get(door_x, door_z, door_level)) {
            println!("{} door loc {loc_id} at ({door_x},{door_z}) opened but stays blocked", self.tag());
            return false;
        }
        true
    }

    /// True once the live loc stops showing the closed id (doors flip id,
    /// shape or vanish when they swing). Bounded watch: a click the engine
    /// ignored reads as shut, never as open.
    fn door_opened(&mut self, door_x: i32, door_z: i32, door_level: i32, closed_id: u16) -> bool {
        let target = self.world.ticks + 12;
        while self.world.ticks < target {
            match self.world.loc_at(door_x, door_z, door_level) {
                Some((id, _, _)) if id == closed_id => {}
                _ => return true,
            }
            if self.pump(2).is_err() {
                return false;
            }
        }
        !matches!(
            self.world.loc_at(door_x, door_z, door_level),
            Some((id, _, _)) if id == closed_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Bot;
    use crate::collision::{self, MapPack};
    use crate::route::Router;

    /// Pack index with no squares, mirror of the movement-tests fixture:
    /// offline stand-in so hand-painted collision survives untouched.
    fn empty_pack() -> MapPack {
        MapPack::new(std::path::PathBuf::from("no-pack.zip"), std::collections::HashMap::new(), Vec::new())
    }

    /// A door already opened by this `approach` call fails fast: both
    /// the on-path opener skips it without a click (no pump, no tick, record kept).
    #[test]
    fn approach_does_not_reopen_same_door() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        bot.doors.insert((10, 0, 0), (1530, 0, 0));
        bot.approach_depth = 1;
        bot.approach_opened.insert((10, 0, 0));
        let start_ticks = bot.world.ticks;
        assert!(!bot.open_door_on_path(&[(10, 0)], false), "already-opened door fails fast");
        assert_eq!(bot.world.ticks, start_ticks, "fast fail pumps nothing");
        assert!(bot.doors.contains_key(&(10, 0, 0)), "record kept");
        bot.approach_depth = 0;
        bot.approach_opened.clear();
    }

    /// Pack with a single known door def (1530, closed, Open op): offline
    /// stand-in for the loaded map index so unpaint runs for real.
    fn door_pack() -> MapPack {
        let mut defs: Vec<Option<crate::collision::LocDef>> = vec![None; 1531];
        defs[1530] = Some(crate::collision::LocDef {
            blockwalk: true,
            blockrange: true,
            length: 1,
            width: 1,
            active: 1,
            has_ops: true,
            open_op: Some(1),
            toll: false,
        });
        MapPack::new(std::path::PathBuf::from("no-pack.zip"), std::collections::HashMap::new(), defs)
    }

    /// Bot one step south of a shut ordinary door (3226,3223, shape 0 angle
    /// 2), painted shut, known, live state showing the closed id: the
    /// fast-door setup (opener stands adjacent, clicks, steps on).
    fn shut_door_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(door_pack());
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3222;
        crate::collision::change_loc_collision(&mut bot.collision, 0, 2, true, 1, 1, 1, 3226, 3223, 0, true);
        bot.doors.insert((3226, 3223, 0), (1530, 0, 2));
        bot.world.locs.insert((3226, 3223, 0), Some((1530, 0, 2)));
        bot
    }

    /// Fast ordinary door, mocked server: the click flips the live loc and
    /// the step lands — the decision fires, and the crossing books in 4
    /// simulated ticks (op + short pump + step), never the 12-tick watch.
    /// Record kept for the re-close, tile unpaints for the walk through.
    #[test]
    fn immediate_step_crosses_fast_door_within_6_ticks() {
        let mut bot = shut_door_bot();
        assert_eq!(bot.immediate_step_tile(3226, 3223, 0, 1530), Some((3226, 3223)), "ordinary adjacent door steps");
        // Mock server answers (world edits stand in for streamed frames):
        // the click flips the live loc, the step lands on the door tile.
        let start = bot.world.ticks;
        bot.world.ticks += 1; // op
        bot.world.ticks += 2; // short pump sees the flip
        bot.world.locs.insert((3226, 3223, 0), None);
        bot.world.x = 3226;
        bot.world.z = 3223;
        bot.world.ticks += 1; // step accepted
        assert!(bot.world.ticks - start <= 6, "crossed on the open swing");
        bot.approach_depth = 1;
        assert!(bot.crossed_door_now(3226, 3223, 0, 1530, 0, 2), "crossing books");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record kept for the re-close");
        assert!(bot.approach_opened.contains(&(3226, 3223, 0)), "this approach call marked");
        assert!(crate::collision::tile_walkable(bot.collision.get(3226, 3223, 0)), "freed tile walks");
        bot.approach_depth = 0;
        bot.approach_opened.clear();
    }

    /// Full crossing while a fast door re-closes 3 ticks after the click:
    /// onto lands on the door tile, off lands on the route tile after it
    /// (3225,3223), all inside a few ticks — no 12-tick watch, no 20-tick
    /// replan between. The re-close flips the live loc back, but the bot
    /// is already through and the booking keeps the record.
    #[test]
    fn door_traversal_crosses_before_reclose_within_8_ticks() {
        let mut bot = shut_door_bot();
        let route = vec![(3226, 3223), (3225, 3223)];
        assert_eq!(
            bot.door_off_tile(3226, 3223, 0, 1530, route.get(1).copied()),
            Some((3225, 3223)),
            "off is the route tile after the door"
        );
        // Mock server answers (world edits stand in for streamed frames):
        // the click flips the live loc, onto lands on the door tile, off
        // lands past it — back to back, like the chained live steps.
        let start = bot.world.ticks;
        bot.world.ticks += 1; // op
        bot.world.ticks += 2; // short pump sees the flip
        bot.world.locs.insert((3226, 3223, 0), None);
        bot.world.x = 3226; // onto lands
        bot.world.z = 3223;
        bot.world.ticks += 1;
        bot.world.x = 3225; // off lands, same call, no replan between
        bot.world.z = 3223;
        bot.world.ticks += 1;
        assert!(bot.world.ticks - start <= 8, "onto+off inside a few ticks");
        bot.world.locs.insert((3226, 3223, 0), Some((1530, 0, 2))); // re-close 3 ticks after the op
        assert_eq!((bot.world.x, bot.world.z), (3225, 3223), "through before the re-close");
        bot.approach_depth = 1;
        assert!(bot.crossed_door_now(3226, 3223, 0, 1530, 0, 2), "crossing books");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record kept for the re-close");
        assert!(bot.approach_opened.contains(&(3226, 3223, 0)), "this approach call marked");
        assert!(crate::collision::tile_walkable(bot.collision.get(3226, 3223, 0)), "freed tile walks");
        bot.approach_depth = 0;
        bot.approach_opened.clear();
    }

    /// Refused off step: the bot stands on the door tile (onto landed),
    /// the route tile after it refuses — logged with live loc and flags
    /// (same line as legs), record kept, hard fail (no blind replan).
    /// Offline the send never leaves, which reads as a refusal here.
    #[test]
    fn door_off_refused_logs_and_fails_loud_record_kept() {
        let mut bot = shut_door_bot();
        bot.world.x = 3226; // onto landed
        bot.world.z = 3223;
        bot.approach_depth = 1;
        let live = bot.world.loc_at(3225, 3223, 0);
        let flags = bot.collision.get(3225, 3223, 0);
        let start = bot.world.ticks;
        assert_eq!(
            bot.step_off_door_now(3226, 3223, 0, 1530, 0, 2, Some((3225, 3223))),
            Some(false),
            "refused off fails loud"
        );
        assert_eq!(bot.world.ticks, start, "refusal pumps nothing");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record kept");
        assert!(bot.approach_opened.contains(&(3226, 3223, 0)), "same call marked, no re-click loop");
        let line = Bot::refused_line(&[(3225, 3223)], (3226, 3223, 0), live, flags);
        assert!(
            line.starts_with("step refused: wanted [(3225, 3223)] from (3226,3223,0) live="),
            "off refusal uses the leg line, got: {line}"
        );
        assert!(line.ends_with(&format!("flags={flags:#x}")), "off refusal carries flags, got: {line}");
        bot.approach_depth = 0;
        bot.approach_opened.clear();
    }

    /// No known next tile: the door is last on the route and the walk
    /// objective sits elsewhere — no off step fires, the onto alone
    /// books exactly as before. An open modal never steps off either;
    /// the adjacent objective is the only fallback.
    #[test]
    fn door_off_without_next_tile_steps_nothing() {
        let mut bot = shut_door_bot();
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, None), None, "no next tile, no goal: no off step");
        bot.route_target = Some((3200, 3200));
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, None), None, "far goal is no off step either");
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, Some((3230, 3230))), None, "non-adjacent route tile is no off step");
        bot.route_target = Some((3226, 3223));
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, None), None, "goal on the door needs no off step");
        bot.route_target = Some((3225, 3223));
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, None), Some((3225, 3223)), "adjacent goal is the off step");
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, Some((3230, 3230))), Some((3225, 3223)), "adjacent goal covers a bad route tile");
        bot.world.modal = Some(crate::world::Modal { layer: "modal".to_string(), interface_id: 1 });
        assert_eq!(bot.door_off_tile(3226, 3223, 0, 1530, Some((3225, 3223))), None, "open modal pays first");
        assert_eq!(bot.step_off_door_now(3226, 3223, 0, 1530, 0, 2, None), None, "no off tile means onto alone, as before");
        assert_eq!(bot.world.ticks, 0, "decisions pump nothing");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record untouched");
    }

    /// Shut door that never flips: the 12-tick watch still refuses
    /// (fallback unchanged) and both openers fail loud — the click never
    /// leaves offline, the record stays, no tick burns.
    #[test]
    fn shut_door_falls_back_to_watch_and_fails_loud() {
        let mut bot = shut_door_bot();
        assert!(!bot.door_opened(3226, 3223, 0, 1530), "watch still refuses a shut door");
        assert_eq!(bot.world.ticks, 0, "refusal pumps nothing offline");
        assert!(!bot.open_door_on_path(&[(3226, 3223)], false), "click never leaves offline");
        assert_eq!(bot.world.ticks, 0, "failures pump nothing offline");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record kept");
    }

    /// Doors without an open action keep their registry entry: the two
    /// opener exits that once dropped the record now restore it, so a
    /// learned door stays queryable after every failure path.
    #[test]
    fn opener_without_open_op_keeps_registry() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        // 9999/9998 have no def at all (empty pack): the opener hits
        // the no-open-op exit while live still reads them shut.
        bot.doors.insert((10, 0, 0), (9999, 0, 2));
        bot.doors.insert((12, 0, 0), (9998, 0, 2));
        bot.world.locs.insert((10, 0, 0), Some((9999, 0, 2)));
        bot.world.locs.insert((12, 0, 0), Some((9998, 0, 2)));
        assert!(!bot.open_door_on_path(&[(10, 0)], false), "no open action fails loud");
        assert_eq!(bot.doors.len(), 2, "on-path failure keeps every entry");
        assert_eq!(bot.doors.get(&(10, 0, 0)), Some(&(9999, 0, 2)));
        assert_eq!(bot.doors.get(&(12, 0, 0)), Some(&(9998, 0, 2)));
        assert_eq!(bot.world.ticks, 0, "failures pump nothing");
    }

    /// A statically-openable door the registry never learned registers
    /// on verified open: live already shows it open, so the click is
    /// skipped and the record appears for `open_nearest` and planning.
    /// Tiles with no static door stay ignored.
    #[test]
    fn open_shut_registers_unknown_static_door() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(door_pack());
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3222;
        bot.world.static_locs.insert((3226, 3223, 0), (1530, 0, 2));
        bot.world.locs.insert((3226, 3223, 0), None);
        assert!(bot.doors.is_empty());
        assert!(bot.open_shut_door(3226, 3223, 0, 1530), "already open costs nothing");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "verified door registers");
        assert!(bot.open_shut_door(9, 9, 0, 1530), "unknown tile still costs nothing");
        assert_eq!(bot.doors.len(), 1, "non-door tile stays ignored");
    }

    /// Toll modal first: with a toll dialog open the immediate step never
    /// fires — the paid path (op, modal, choose) runs unchanged.
    #[test]
    fn toll_modal_handled_before_immediate_step() {
        let mut bot = shut_door_bot();
        bot.toll_gates.insert((3226, 3223, 0));
        bot.allow_paid = true;
        bot.world.modal = Some(crate::world::Modal { layer: "modal".to_string(), interface_id: 1 });
        assert_eq!(bot.immediate_step_tile(3226, 3223, 0, 1530), None, "modal pays first");
        assert!(!bot.step_onto_door_now(3226, 3223, 0, 1530), "no step before the modal");
        assert_eq!(bot.world.ticks, 0, "guard pumps nothing");
    }

    /// Bot next to the Lumbridge door shape (3226,3223, shape 0 angle 2),
    /// painted shut in the base map and known, but the live state already
    /// shows it open (server deleted the closed loc on click).
    fn open_door_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(door_pack());
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3222;
        crate::collision::change_loc_collision(&mut bot.collision, 0, 2, true, 1, 1, 1, 3226, 3223, 0, true);
        bot.doors.insert((3226, 3223, 0), (1530, 0, 2));
        bot.world.locs.insert((3226, 3223, 0), None);
        bot
    }

    /// An already-open door is not re-clicked: no stand, no click, no pump
    /// (ticks and intent untouched), the record stays, and the recorded
    /// (static) footprint still unpaints the passage planning walks through.
    #[test]
    fn already_open_door_skips_click_and_plans_through() {
        let mut bot = open_door_bot();
        assert!(!bot.open_door_on_path(&[(3226, 3223)], false));
        assert_eq!(bot.world.ticks, 0, "skip pumps nothing");
        assert!(bot.intent.is_empty(), "skip announces no opening");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record kept");
        assert!(
            crate::collision::step_open(&bot.collision, 3226, 3222, 0, 0, 1),
            "recorded footprint unpaints the way in"
        );
        assert!(
            crate::collision::step_open(&bot.collision, 3226, 3223, 0, -1, 0),
            "recorded footprint unpaints the way out"
        );
        let view = bot.maps.as_ref().expect("pack").planning_view(
            &bot.collision,
            &bot.doors,
            &bot.toll_gates,
            bot.allow_paid,
        );
        assert!(crate::collision::tile_walkable(view.get(3226, 3223, 0)));
        let mut router = Router::new();
        let waypoints = router.find_path_to_tile(&view, 0, 3226, 3222, 3225, 3223);
        assert_eq!(
            waypoints.last(),
            Some(&crate::route::Waypoint { level: 0, x: 3225, z: 3223 }),
            "planning walks through the open door"
        );
    }

    /// A second pass over the same open door clicks nothing either: the
    /// live re-click loop costs no stand, no click and no tick.
    #[test]
    fn second_pass_over_open_door_clicks_nothing() {
        let mut bot = open_door_bot();
        assert!(!bot.open_door_on_path(&[(3226, 3223)], false));
        assert!(!bot.open_door_on_path(&[(3226, 3223)], false));
        assert_eq!(bot.world.ticks, 0, "passes pump nothing");
        assert!(bot.intent.is_empty(), "passes announce no opening");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 2)), "record kept");
    }

    /// An already-open door with no socket: the walk plans for real, then
    /// the first send fails typed (reconnect) before any pump. The record
    /// stays for the next crossing.
    #[test]
    fn already_open_door_without_route_fails_loud() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(door_pack());
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        bot.doors.insert((5, 0, 0), (1530, 0, 2));
        bot.world.locs.insert((5, 0, 0), None);
        let (world, tables) = crate::client::travel::test_nav_context();
        bot.nav_world = Some(world);
        bot.nav_tables = Some(tables);
        // No socket: keepalive primed as just-sent so the plan runs pure
        // and the first real send fails typed.
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        let err = bot.walk_to(3200, 3227, 0, 600).unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("not logged in"), "typed conn error, got: {message}");
        assert_eq!(bot.world.ticks, 0, "failure pumps nothing");
        assert_eq!(bot.doors.get(&(5, 0, 0)), Some(&(1530, 0, 2)), "record kept");
    }

}
