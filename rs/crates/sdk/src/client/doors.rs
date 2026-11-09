//! Doors: open, verify and walk through them like the manual player
//! does — adjacent, click, step through at once, no speculative opens
//! ahead and no silent pacing. Planning already carries the click
//! (`open_op`, toll) on its segments; this module only bridges the live
//! gap (click, verify, step). The worker keeps no painted map: the door
//! registry is seeded per plan run from the plan plus live checks, and
//! off-plan clicks read their def from the service. Splitting rule of the
//! repo: one role per module, no file above 1000 lines.

use lifescape_nav::plan::{Plan, Segment};

use crate::client::Bot;
use crate::error::Result;

impl Bot {
    /// Seed the ephemeral registry for one plan run: every `OpenDoor`
    /// segment lands here with its toll flag from the plan and its live
    /// geometry when streamed (`0,0` when not). Cleared on run exit:
    /// nothing persists between runs, nothing is ever painted.
    pub(crate) fn seed_plan_doors(&mut self, plan: &Plan) {
        self.doors.clear();
        self.toll_gates.clear();
        for segment in &plan.segments {
            if let Segment::OpenDoor { door, .. } = segment {
                let (_, shape, angle) =
                    self.world.loc_at(door.x, door.z, door.level).unwrap_or((door.loc, 0, 0));
                self.doors.insert((door.x, door.z, door.level), (door.loc, shape, angle));
                if door.toll {
                    self.toll_gates.insert((door.x, door.z, door.level));
                }
            }
        }
    }

    /// Drop the run registry: the next run seeds its own doors.
    pub(crate) fn clear_plan_doors(&mut self) {
        self.doors.clear();
        self.toll_gates.clear();
    }

    /// Open one known shut door we stand next to (walk legs step one
    /// tile at a time, so adjacency holds): click and verify live.
    /// Already-open (live id flipped) costs nothing. The open op and toll
    /// come from the service def (no pack worker-side); a down service is
    /// `Error::Travel("nav-down: ...")`, never a guessed click. True when
    /// the step may go through; toll gates without allowance stay shut.
    pub fn open_shut_door(&mut self, door_x: i32, door_z: i32, door_level: i32, loc_id: u16) -> Result<bool> {
        if self.toll_gates.contains(&(door_x, door_z, door_level)) && !self.allow_paid {
            return Ok(false);
        }
        // Already open costs nothing: the live loc left the closed id. A
        // cleared tile (removed) is a verified open door, an unstreamed
        // one is simply not a click — both cost nothing, only the removed
        // one joins the run registry.
        if !matches!(self.world.loc_at(door_x, door_z, door_level), Some((id, _, _)) if id == loc_id) {
            if self.world.loc_removed(door_x, door_z, door_level) {
                self.doors.insert((door_x, door_z, door_level), (loc_id, 0, 0));
            } else if let Some((_, shape, angle)) = self.world.loc_at(door_x, door_z, door_level) {
                self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            }
            return Ok(true);
        }
        let def = match self.door_defs(std::slice::from_ref(&loc_id)) {
            Ok(mut defs) => defs.pop(),
            Err(error) => {
                self.log(&format!("door loc {loc_id} at ({door_x},{door_z}) defs unavailable: {error}"));
                return Err(error);
            }
        };
        let Some(def) = def else {
            return Ok(false);
        };
        if def.toll && !self.allow_paid {
            return Ok(false);
        }
        if def.toll {
            self.toll_gates.insert((door_x, door_z, door_level));
        }
        let Some(op) = def.open_op else {
            return Ok(false);
        };
        self.intend(&format!("opening the door at ({door_x},{door_z})"));
        if self.op_loc(op, door_x, door_z, loc_id).is_err() {
            return Ok(false);
        }
        if !self.door_opened(door_x, door_z, door_level, loc_id) {
            println!("{} door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut", self.tag());
            return Ok(false);
        }
        let (_, shape, angle) =
            self.world.loc_at(door_x, door_z, door_level).unwrap_or((loc_id, 0, 0));
        self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        Ok(true)
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
    use crate::client::travel::test_nav_context;
    use crate::error::Error;
    use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};

    /// Off-refusal line shape, locked: same leg line as walks, live loc on
    /// the refused tile, batch flags — `unknown` when the batch was down.
    #[test]
    fn door_off_refusal_line_matches_legs() {
        let live = Some((1530, 0, 2));
        let line = Bot::refused_line(&[(3225, 3223)], (3226, 3223, 0), live, Some(0x240100));
        assert!(
            line.starts_with("step refused: wanted [(3225, 3223)] from (3226,3223,0) live="),
            "off refusal uses the leg line, got: {line}"
        );
        assert!(line.ends_with("flags=0x240100"), "off refusal carries flags, got: {line}");
        let down = Bot::refused_line(&[(3225, 3223)], (3226, 3223, 0), live, None);
        assert!(down.ends_with("flags=unknown"), "down batch keeps the live half, got: {down}");
    }

    /// A cleared tile (live removed) is a verified open door: the click is
    /// skipped and the record appears without any service round-trip. A
    /// merely unstreamed tile costs nothing too but records nothing.
    #[test]
    fn open_shut_registers_verified_open_only() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3222;
        bot.world.locs.insert((3226, 3223, 0), None);
        assert!(bot.doors.is_empty());
        assert!(bot.open_shut_door(3226, 3223, 0, 1530).expect("removed tile costs nothing"), "removed tile costs nothing");
        assert_eq!(bot.doors.get(&(3226, 3223, 0)), Some(&(1530, 0, 0)), "verified door registers");
        assert!(bot.open_shut_door(9, 9, 0, 1530).expect("unstreamed tile costs nothing"), "unstreamed tile still costs nothing");
        assert_eq!(bot.doors.len(), 1, "unstreamed tile records nothing");
    }

    /// Shut off-plan door with a down service: the def never arrives, so
    /// the call fails `nav-down` loud (logged, no guessed click). Same
    /// call with the service up clicks its def op instead.
    #[test]
    fn shut_off_plan_door_needs_its_service_def() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3222;
        bot.world.locs.insert((3226, 3223, 0), Some((1530, 0, 2)));
        // No handle: the click never leaves, `nav-down` fails loud.
        let err = bot.open_shut_door(3226, 3223, 0, 1530).expect_err("down service fails nav-down");
        assert!(
            matches!(&err, Error::Travel(detail) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {err}"
        );
        assert!(bot.doors.get(&(3226, 3223, 0)).is_none(), "unguessed door records nothing");
        // Real defs, dead socket: the click still fails typed (Login), and
        // the lookup itself proves the def arrived (toll read, op read).
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "door-def".to_string()),
            &test_socket("sdk-door-def"),
            1,
        )
        .expect("service starts");
        let mut served = Bot::new("127.0.0.1", 43594, ".", ".");
        served.world.level = 0;
        served.world.x = 3226;
        served.world.z = 3222;
        served.world.locs.insert((3226, 3223, 0), Some((1530, 0, 2)));
        served.plan_client = Some(PlanClient::new(service.socket_path(), "door-def"));
        served.plan_data_version = Some("door-def".to_string());
        served.last_keepalive = Some(std::time::Instant::now());
        served.last_send_tick = served.world.ticks;
        // Loc 1530 is a real openable door: the def carries its click.
        let defs = served.door_defs(&[1530]).expect("defs answer");
        assert!(defs[0].open_op.is_some(), "real door carries its open op");
    }

    /// A bare worker (no map, no local world, no handle) never plans
    /// local: the walk fails loud with `nav-down` before any pump.
    #[test]
    fn already_open_door_without_route_fails_loud() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        // No socket: keepalive primed as just-sent so the failure is the
        // missing handle, never a conn error mid-plan.
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        let err = bot.walk_to(3200, 3227, 0, 600).unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("nav-down"), "missing handle is nav-down, got: {message}");
        assert!(matches!(err, Error::Travel(_)), "typed travel error, got: {err:?}");
        assert_eq!(bot.world.ticks, 0, "failure pumps nothing");
    }
}
