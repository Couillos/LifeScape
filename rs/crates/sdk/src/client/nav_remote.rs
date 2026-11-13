//! Remote direct reads for mapless workers: door defs and tile flags.
//!
//! One round-trip per batch through the plan socket, no queue ticket, no
//! admission: the reads answer off the shared snapshot. The pinned data
//! version resubmits once on skew, then `nav-down` — the same discipline
//! as plans, never a local read.
//!
//! Only `wants` is cached worker-side (pinned by data version): it feeds
//! every plan, so each plan would otherwise pay an extra round-trip
//! before submitting. Every other read (`door_defs`, `tiles_flags`,
//! `static_locs_near`, `in_f2p`, `toll_fence`, `transitions_list`,
//! `probe_targets`, `doors_near`, `debug_snapshot`) fetches fresh on each
//! call under the same version check + one resubmit: they run at most
//! once per leg, plan or diagnosis, so the extra trip buys guaranteed
//! post-reload freshness with no per-op invalidation state to keep.

use lifescape_json::{object, str_value, Value};
use lifescape_nav::plan::{NavOptions, Tile};
use lifescape_nav::service::PlanClient;
use lifescape_nav::service_queries::{self, DebugAnswer, DoorDef, StaticLoc, TileFlags};

pub use lifescape_nav::service_queries::{ProbeTarget, TransitionView};

use std::time::Instant;

use lifescape_nav::service::SERVICE_WAIT_MAX;

use super::service_wait::service_stalled;
use super::travel::stale_reply;
use crate::client::Bot;
use crate::error::{Error, Result};

impl Bot {
    fn service_handle(&self) -> Result<PlanClient> {
        self.plan_client
            .clone()
            .ok_or_else(|| Error::Travel("nav-down: no-service-handle".to_string()))
    }

    /// Defs for a batch of loc ids (off-plan clicks): open op, toll and
    /// footprint fields, in request order. Unknown ids come back shut and
    /// never-paid. Any service failure is `nav-down`, never a guess.
    pub fn door_defs(&mut self, loc_ids: &[u16]) -> Result<Vec<DoorDef>> {
        if loc_ids.is_empty() {
            return Ok(Vec::new());
        }
        let locs = loc_ids.iter().map(|loc| Value::Int(i64::from(*loc))).collect();
        let reply = self.query_remote("door_defs", vec![("locs", Value::Arr(locs))])?;
        service_queries::decode_door_defs_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad door_defs reply: {error}")))
    }

    /// Static flags for a batch of tiles (diagnosis, stepping), in request
    /// order. Any service failure is `nav-down`, never a local read.
    pub fn tiles_flags(&mut self, tiles: &[(i32, i32, i32)]) -> Result<Vec<TileFlags>> {
        if tiles.is_empty() {
            return Ok(Vec::new());
        }
        let coords = tiles
            .iter()
            .map(|(x, z, level)| {
                Value::Arr(vec![
                    Value::Int(i64::from(*x)),
                    Value::Int(i64::from(*z)),
                    Value::Int(i64::from(*level)),
                ])
            })
            .collect();
        let reply = self.query_remote("tiles_flags", vec![("tiles", Value::Arr(coords))])?;
        service_queries::decode_tiles_flags_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad tiles_flags reply: {error}")))
    }

    /// Batch flags that never fail the caller: `None` when the service is
    /// down. Diagnosis only — the step already failed, so the line keeps
    /// its live half instead of failing the failure.
    pub(crate) fn tile_flags_best_effort(&mut self, tiles: &[(i32, i32, i32)]) -> Option<Vec<TileFlags>> {
        self.tiles_flags(tiles).ok()
    }

    /// Static locs within a Manhattan radius on one level, nearest first:
    /// id plus shape/angle, names resolved worker-side. Replaces
    /// worker-side `static_locs` scans (`find_loc`, booths, `gate_id`).
    /// Any service failure is `nav-down`, never a local scan.
    pub fn static_locs_near(&mut self, x: i32, z: i32, level: i32, radius: i32) -> Result<Vec<StaticLoc>> {
        let reply = self.query_remote(
            "static_locs_near",
            vec![
                ("x", Value::Int(i64::from(x))),
                ("z", Value::Int(i64::from(z))),
                ("level", Value::Int(i64::from(level))),
                ("radius", Value::Int(i64::from(radius))),
            ],
        )?;
        service_queries::decode_static_locs_near_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad static_locs_near reply: {error}")))
    }

    /// One F2P verdict per tile, in request order. Replaces the spot
    /// filters that read the painted world. Empty is free, like the
    /// other batches.
    pub fn in_f2p_batch(&mut self, tiles: &[(i32, i32, i32)]) -> Result<Vec<bool>> {
        if tiles.is_empty() {
            return Ok(Vec::new());
        }
        let coords = tiles
            .iter()
            .map(|(x, z, level)| {
                Value::Arr(vec![
                    Value::Int(i64::from(*x)),
                    Value::Int(i64::from(*z)),
                    Value::Int(i64::from(*level)),
                ])
            })
            .collect();
        let reply = self.query_remote("in_f2p", vec![("tiles", Value::Arr(coords))])?;
        service_queries::decode_in_f2p_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad in_f2p reply: {error}")))
    }

    /// Nearest toll-gate tile on this level within the radius, if any.
    /// Its x is the fence line banks and spots compare against; the tile
    /// itself is the approach point. Replaces `toll_gate_x`.
    pub fn toll_fence(&mut self, x: i32, z: i32, level: i32, radius: i32) -> Result<Option<(i32, i32, i32)>> {
        let reply = self.query_remote(
            "toll_fence",
            vec![
                ("x", Value::Int(i64::from(x))),
                ("z", Value::Int(i64::from(z))),
                ("level", Value::Int(i64::from(level))),
                ("radius", Value::Int(i64::from(radius))),
            ],
        )?;
        service_queries::decode_toll_fence_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad toll_fence reply: {error}")))
    }

    /// Every tag a plan may require, sorted and distinct, under the
    /// served version. The worker satisfies them from live state
    /// (coins, quest points, carried items) to build its options.
    pub fn wants(&mut self) -> Result<Vec<String>> {
        let reply = self.query_remote("wants", Vec::new())?;
        service_queries::decode_wants_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad wants reply: {error}")))
    }

    /// Flags here/dest, template locs near dest and the plan (or NoRoute)
    /// for the same inputs `travel_to` plans with: the static half of
    /// `debug_walk`, one round trip, same skew discipline as a plan.
    pub fn debug_snapshot(&mut self, from: Tile, dest: Tile, options: &NavOptions) -> Result<DebugAnswer> {
        let reply = self.query_remote(
            "debug",
            vec![
                ("from", object(vec![
                    ("x", Value::Int(i64::from(from.x))),
                    ("z", Value::Int(i64::from(from.z))),
                    ("level", Value::Int(i64::from(from.level))),
                ])),
                ("dest", object(vec![
                    ("x", Value::Int(i64::from(dest.x))),
                    ("z", Value::Int(i64::from(dest.z))),
                    ("level", Value::Int(i64::from(dest.level))),
                ])),
                ("options", lifescape_nav::service::encode_options(options)),
            ],
        )?;
        service_queries::decode_debug_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad debug reply: {error}")))
    }

    /// Every transition row, in table order, under the served version.
    /// Replaces file reads in admin checks; disabled rows included.
    pub fn transitions_list(&mut self) -> Result<Vec<TransitionView>> {
        let reply = self.query_remote("transitions_list", Vec::new())?;
        service_queries::decode_transitions_list_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad transitions_list reply: {error}")))
    }

    /// Stands and expects for the filtered rows, computed service-side
    /// with the prober options. `None` stand means no reach-valid stand:
    /// the worker logs the skip, exactly like the local computation.
    pub fn probe_targets(&mut self, filter: Option<&str>) -> Result<Vec<ProbeTarget>> {
        let reply = self.query_remote(
            "probe_targets",
            vec![("filter", filter.map_or(Value::Null, str_value))],
        )?;
        service_queries::decode_probe_targets_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad probe_targets reply: {error}")))
    }

    /// Template doors within the radius, nearest first. Replaces worker
    /// scans for admin door checks; names resolve worker-side.
    pub fn doors_near(&mut self, x: i32, z: i32, level: i32, radius: i32) -> Result<Vec<StaticLoc>> {
        let reply = self.query_remote(
            "doors_near",
            vec![
                ("x", Value::Int(i64::from(x))),
                ("z", Value::Int(i64::from(z))),
                ("level", Value::Int(i64::from(level))),
                ("radius", Value::Int(i64::from(radius))),
            ],
        )?;
        service_queries::decode_static_locs_near_reply(&reply)
            .map_err(|error| Error::Travel(format!("nav-down: bad doors_near reply: {error}")))
    }

    /// One direct read off a shared reference: skew adopts the served
    /// version locally and retries once, any failure is `None`. Dossier
    /// and diagnosis only — the caller keeps its live half instead of
    /// inventing static state, and the next plan adopts the version.
    pub(crate) fn direct_best_effort(&self, op: &str, body: Vec<(&str, Value)>) -> Option<Value> {
        let client = self.plan_client.clone()?;
        let mut version = self.plan_data_version.clone().unwrap_or_default();
        let mut resubmits = 0u32;
        loop {
            let mut fields = vec![("op", str_value(op)), ("version", str_value(&version))];
            fields.extend(body.clone());
            let reply = match client.request(&object(fields)) {
                Ok(reply) => reply,
                Err(error) => {
                    self.log(&format!("{op} unavailable: {error}"));
                    return None;
                }
            };
            if reply.get("ok").as_bool() != Some(true) {
                let reason = reply.get("error").as_str().unwrap_or("refused").to_string();
                let served = reply.get("version").as_str().unwrap_or("").to_string();
                if reason.starts_with("refused:stale-version") && resubmits < 1 && !served.is_empty() {
                    version = served;
                    resubmits += 1;
                    continue;
                }
                self.log(&format!("{op} refused: {reason}"));
                return None;
            }
            let served = reply.get("version").as_str().unwrap_or("").to_string();
            if stale_reply(&self.plan_data_version, &served) {
                if resubmits >= 1 {
                    self.log(&format!("{op} skew twice, dropping"));
                    return None;
                }
                version = served;
                resubmits += 1;
                continue;
            }
            return Some(reply);
        }
    }

    /// One direct read with the pinned version: skew adopts the served
    /// version and retries once, then `nav-down`. No ticket, no queue.
    /// The game session stays fed during the round trip (`serve_fed`); a
    /// stalled round trip (no answer in time) is retried until the
    /// service guard, then `nav-down` naming the stalls.
    fn query_remote(&mut self, op: &str, body: Vec<(&str, Value)>) -> Result<Value> {
        let client = self.service_handle()?;
        let started = Instant::now();
        let mut resubmits = 0u32;
        let mut stalls = 0u32;
        loop {
            let version = self.plan_data_version.clone().unwrap_or_default();
            let mut fields = vec![("op", str_value(op)), ("version", str_value(&version))];
            fields.extend(body.clone());
            let request = object(fields);
            let caller = client.clone();
            let reply = match self.serve_fed(move || caller.request(&request))? {
                Ok(reply) => reply,
                Err(error) if service_stalled(&error) && started.elapsed() < SERVICE_WAIT_MAX => {
                    stalls += 1;
                    self.log(&format!("{op}: service stalled ({error}), retry {stalls}"));
                    if !self.plan_yield() {
                        return Err(Error::Interrupted(format!("{op} aborted at {:?}", self.position())));
                    }
                    continue;
                }
                Err(error) if service_stalled(&error) => {
                    return Err(Error::Travel(format!(
                        "nav-down: {op} stalled {} times over {}s: {error}",
                        stalls + 1,
                        started.elapsed().as_secs()
                    )));
                }
                Err(error) => return Err(Error::Travel(format!("nav-down: {error}"))),
            };
            if reply.get("ok").as_bool() != Some(true) {
                let reason = reply.get("error").as_str().unwrap_or("refused").to_string();
                let served = reply.get("version").as_str().unwrap_or("").to_string();
                if reason.starts_with("refused:stale-version") && resubmits < 1 && !served.is_empty() {
                    self.plan_data_version = Some(served);
                    resubmits += 1;
                    continue;
                }
                return Err(Error::Travel(format!("nav-down: {reason}")));
            }
            let served = reply.get("version").as_str().unwrap_or("").to_string();
            if stale_reply(&self.plan_data_version, &served) {
                self.plan_data_version = Some(served.clone());
                if resubmits >= 1 {
                    return Err(Error::Travel(format!("nav-down: version-skew server={served}")));
                }
                resubmits += 1;
                continue;
            }
            return Ok(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_nav::plan::Tile;
    use lifescape_nav::service::{Service, Snapshot, test_socket};

    use crate::client::travel::test_nav_context;

    fn test_options() -> lifescape_nav::plan::NavOptions {
        lifescape_nav::plan::NavOptions {
            allow_paid: false,
            f2p_only: true,
            satisfied: std::collections::HashSet::new(),
            excluded_edges: std::collections::HashSet::new(),
            danger: Vec::new(),
        }
    }

    fn served_bot(account: &str, socket: &std::path::Path, version: &str) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.x = 3228;
        bot.world.z = 3214;
        bot.world.level = 0;
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(socket, account));
        bot.plan_data_version = Some(version.to_string());
        bot
    }

    #[test]
    fn door_defs_match_the_planned_click() {
        let (world, tables) = test_nav_context();
        let version = "remote-q";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-qdefs"), 1)
            .expect("service starts");
        let mut bot = served_bot("qdefs", service.socket_path(), version);
        // Bare bot, no local map: plan remote, then defs for its door.
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let (_, plan, _) = bot.plan_travel(dest).expect("bare bot plans remote");
        let door = plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                lifescape_nav::plan::Segment::OpenDoor { door, .. } => Some(door.clone()),
                _ => None,
            })
            .expect("door leg swings a door");
        let defs = bot.door_defs(&[door.loc]).expect("defs answer");
        assert_eq!(defs.len(), 1, "one def per id, in order");
        assert_eq!(defs[0].open_op, door.open_op, "def matches the planned click");
        assert_eq!(defs[0].toll, door.toll, "def matches the planned toll");
        // Empty batch costs no round-trip, even without a handle.
        bot.plan_client = None;
        assert!(bot.door_defs(&[]).expect("empty is free").is_empty());
        assert!(bot.tiles_flags(&[]).expect("empty is free").is_empty());
    }

    #[test]
    fn tiles_flags_name_the_door_tile() {
        let (world, tables) = test_nav_context();
        let version = "remote-qflags";
        let service =
            Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-qflags"), 1)
                .expect("service starts");
        let mut bot = served_bot("qflags", service.socket_path(), version);
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let (_, plan, _) = bot.plan_travel(dest).expect("bare bot plans remote");
        let door = plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                lifescape_nav::plan::Segment::OpenDoor { door, .. } => Some(door.clone()),
                _ => None,
            })
            .expect("door leg swings a door");
        let rows = bot.tiles_flags(&[(door.x, door.z, door.level)]).expect("flags answer");
        assert_eq!(rows.len(), 1, "one row per tile, in order");
        assert_eq!((rows[0].tile.x, rows[0].tile.z, rows[0].tile.level), (door.x, door.z, door.level));
        assert!(rows[0].door.is_some(), "door tile names its door");
        assert_eq!(rows[0].toll, door.toll, "toll matches the planned flag");
        let (_, flags_served) = service.query_stats();
        assert!(flags_served >= 1, "the batch counted server-side");
    }

    #[test]
    fn static_locs_near_names_the_planned_door() {
        let (world, tables) = test_nav_context();
        let version = "remote-near";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-near"), 1)
            .expect("service starts");
        let mut bot = served_bot("near", service.socket_path(), version);
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let (_, plan, _) = bot.plan_travel(dest).expect("bare bot plans remote");
        let door = plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                lifescape_nav::plan::Segment::OpenDoor { door, .. } => Some(door.clone()),
                _ => None,
            })
            .expect("door leg swings a door");
        let found = bot.static_locs_near(door.x, door.z, door.level, 2).expect("locs answer");
        assert!(found.iter().any(|entry| entry.loc == door.loc), "the door tile names its loc");
        assert!(found.iter().all(|entry| entry.level == door.level), "level filters");
        let (near, _, _, _) = service.query_stats_scripts();
        assert!(near >= 1, "the batch counted server-side");
    }

    #[test]
    fn in_f2p_batch_matches_the_direct_read() {
        let (world, tables) = test_nav_context();
        let version = "remote-f2p";
        let service = Service::start(Snapshot::new(world.clone(), tables, version.to_string()), &test_socket("sdk-f2p"), 1)
            .expect("service starts");
        let mut bot = served_bot("f2p", service.socket_path(), version);
        let tiles = vec![(3091, 3242, 0), (2862, 3385, 0), (3228, 3214, 0)];
        let inside = bot.in_f2p_batch(&tiles).expect("verdicts answer");
        assert_eq!(inside.len(), tiles.len(), "one verdict per tile, in order");
        for ((x, z, _), flag) in tiles.iter().zip(inside.iter()) {
            assert_eq!(*flag, world.in_f2p(*x, *z), "batch equals the direct read");
        }
        assert!(inside[0], "Draynor bank booth is inside F2P");
        bot.plan_client = None;
        assert!(bot.in_f2p_batch(&[]).expect("empty is free").is_empty());
    }

    #[test]
    fn toll_fence_marks_the_gate_line() {
        let (world, tables) = test_nav_context();
        let version = "remote-fence";
        let service =
            Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-fence"), 1)
                .expect("service starts");
        let mut bot = served_bot("fence", service.socket_path(), version);
        let gate = bot.toll_fence(3265, 3227, 0, 30).expect("fence answers");
        let Some((gate_x, gate_z, gate_level)) = gate else {
            panic!("a fence stands near the toll corridor");
        };
        assert_eq!(gate_level, 0, "same level");
        assert!((gate_x - 3265).abs() + (gate_z - 3227).abs() <= 30, "within the radius");
        assert!(bot.toll_fence(3091, 3242, 0, 10).expect("far answers").is_none(), "no fence near Draynor bank");
        let (_, _, fences, _) = service.query_stats_scripts();
        assert!(fences >= 2, "both batches counted server-side");
    }

    #[test]
    fn wants_lists_every_planner_tag() {
        let (world, tables) = test_nav_context();
        let version = "remote-wants";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-wants"), 1)
            .expect("service starts");
        let mut bot = served_bot("wants", service.socket_path(), version);
        let tags = bot.wants().expect("tags answer");
        assert!(tags.contains(&"qp:32".to_string()), "the guild door wants quest points");
        let mut sorted = tags.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(tags, sorted, "sorted and distinct");
        let (_, _, _, served) = service.query_stats_scripts();
        assert!(served >= 1, "the batch counted server-side");
    }

    #[test]
    fn scripts_down_is_nav_down_not_a_guess() {
        let mut bot = served_bot("scriptdown", &test_socket("absent"), "v");
        let err = bot.static_locs_near(3228, 3214, 0, 2).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "locs fail loud, got {err:?}");
        let err = bot.in_f2p_batch(&[(3228, 3214, 0)]).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "f2p fails loud, got {err:?}");
        let err = bot.toll_fence(3265, 3227, 0, 30).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "fence fails loud, got {err:?}");
        let err = bot.wants().unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "wants fails loud, got {err:?}");
    }

    #[test]
    fn scripts_stale_version_resubmits_once() {
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "served-scripts".to_string()),
            &test_socket("sdk-scriptskew"),
            1,
        )
        .expect("service starts");
        let mut bot = served_bot("scriptskew", service.socket_path(), "stale-scripts");
        let tags = bot.wants().expect("skew resubmits, never fails");
        assert!(!tags.is_empty(), "tags land after the resubmit");
        assert_eq!(bot.plan_data_version.as_deref(), Some("served-scripts"), "worker adopts the served version");
    }

    #[test]
    fn scripts_bad_payload_refuses_typed() {
        // A refused op (bad payload, stale skew) surfaces nav-down, never
        // a local read.
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "scriptbad".to_string()),
            &test_socket("sdk-scriptbad"),
            1,
        )
        .expect("service starts");
        let bot = served_bot("scriptbad", service.socket_path(), "scriptbad");
        let client = bot.plan_client.clone().expect("handle");
        for (op, body) in [
            ("static_locs_near", vec![]),
            ("toll_fence", vec![]),
            (
                "in_f2p",
                vec![("tiles", Value::Arr(vec![Value::Int(1)]))],
            ),
        ] {
            let mut fields = vec![("op", str_value(op)), ("version", str_value("scriptbad"))];
            fields.extend(body);
            let reply = client.request(&object(fields)).expect("transport answers");
            assert_eq!(reply.get("ok").as_bool(), Some(false), "{op} refuses, got {reply:?}");
        }
        // Wants takes no params: a stale client version refuses stale.
        let reply = client
            .request(&object(vec![("op", str_value("wants")), ("version", str_value("bogus"))]))
            .expect("transport answers");
        assert_eq!(reply.get("ok").as_bool(), Some(false), "stale wants refuses, got {reply:?}");
    }

    #[test]
    fn debug_snapshot_matches_the_remote_plan() {
        let (world, tables) = test_nav_context();
        let version = "remote-debug";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-debug"), 1)
            .expect("service starts");
        let mut bot = served_bot("debug", service.socket_path(), version);
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let from = Tile { x: bot.world.x, z: bot.world.z, level: bot.world.level };
        let options = test_options();
        let answer = bot.debug_snapshot(from, dest, &options).expect("debug answers");
        let (_, plan, _) = bot.plan_travel(dest).expect("bare bot plans remote");
        let planned = match answer.outcome {
            Ok(planned) => planned,
            Err(no) => panic!("door leg diagnoses reachable, got {}", no.reason.as_str()),
        };
        assert_eq!(planned.segments.len(), plan.segments.len(), "one round trip plans like the queue");
        assert_eq!(planned.cost, plan.cost, "same cost");
        assert!(!answer.locs.is_empty(), "template locs near dest");
        assert!(answer.defs.iter().any(|def| def.open_op.is_some()), "a def names its click");
        let (_, debug, _, _, _) = service.query_stats_diag();
        assert!(debug >= 1, "the batch counted server-side");
    }

    #[test]
    fn debug_snapshot_reports_noroute_without_world() {
        let (world, tables) = test_nav_context();
        let version = "remote-nonoute";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-nonroute"), 1)
            .expect("service starts");
        let mut bot = served_bot("noroute", service.socket_path(), version);
        bot.world.x = 3191;
        bot.world.z = 3366;
        let from = Tile { x: 3191, z: 3366, level: 0 };
        let dest = Tile { x: 3191, z: 3361, level: 0 };
        let options = test_options();
        let answer = bot.debug_snapshot(from, dest, &options).expect("debug answers");
        assert!(matches!(answer.outcome, Err(_)), "guild door diagnoses, never plans");
    }

    #[test]
    fn diag_down_is_nav_down_not_a_guess() {
        let mut bot = served_bot("diagdown", &test_socket("absent"), "v");
        let from = Tile { x: 0, z: 0, level: 0 };
        let options = test_options();
        let err = bot.debug_snapshot(from, from, &options).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "debug fails loud, got {err:?}");
        let err = bot.transitions_list().unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "list fails loud, got {err:?}");
        let err = bot.probe_targets(None).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "targets fail loud, got {err:?}");
        let err = bot.doors_near(0, 0, 0, 5).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "doors fail loud, got {err:?}");
    }

    #[test]
    fn diag_stale_version_resubmits_once() {
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "served-diag".to_string()),
            &test_socket("sdk-diagskew"),
            1,
        )
        .expect("service starts");
        let mut bot = served_bot("diagskew", service.socket_path(), "stale-diag");
        let rows = bot.transitions_list().expect("skew resubmits, never fails");
        assert!(!rows.is_empty(), "rows land after the resubmit");
        assert_eq!(bot.plan_data_version.as_deref(), Some("served-diag"), "worker adopts the served version");
        let (_, _, lists, _, _) = service.query_stats_diag();
        assert_eq!(lists, 1, "only the served resubmit counts");
    }

    #[test]
    fn down_service_is_nav_down_not_a_guess() {
        let mut bot = served_bot("qdown", &test_socket("absent"), "v");
        let err = bot.door_defs(&[1530]).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "defs fail loud, got {err:?}");
        let err = bot.tiles_flags(&[(3226, 3223, 0)]).unwrap_err();
        assert!(matches!(&err, Error::Travel(detail) if detail.contains("nav-down")), "flags fail loud, got {err:?}");
    }

    #[test]
    fn stale_version_resubmits_once() {
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "served-q".to_string()),
            &test_socket("sdk-qskew"),
            1,
        )
        .expect("service starts");
        let mut bot = served_bot("qskew", service.socket_path(), "stale-q");
        let rows = bot.tiles_flags(&[(3228, 3214, 0)]).expect("skew resubmits, never fails");
        assert_eq!(rows.len(), 1);
        assert_eq!(bot.plan_data_version.as_deref(), Some("served-q"), "worker adopts the served version");
    }

    #[test]
    fn unknown_ticket_shape_is_still_typed() {
        // A refused op (bad payload) surfaces nav-down, never a local read.
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "qbad".to_string()),
            &test_socket("sdk-qbad"),
            1,
        )
        .expect("service starts");
        let bot = served_bot("qbad", service.socket_path(), "qbad");
        let client = bot.plan_client.clone().expect("handle");
        let reply = client
            .request(&object(vec![("op", str_value("door_defs")), ("version", str_value("qbad"))]))
            .expect("transport answers");
        assert_eq!(reply.get("ok").as_bool(), Some(false), "missing locs refuses, got {reply:?}");
    }
}
