//! Bot core: connection lifecycle, tick pump and live snapshots.
//!
//! Movement and actions live in sibling modules; this module owns the
//! session (login, pump, snapshot, close).

pub mod actions;
pub mod approach;
pub mod cost_cache;
pub mod doors;
pub mod movement;
pub mod nav_remote;
pub mod snapshot_view;
pub mod travel;
pub mod travel_dossier;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::collision::{CollisionMap, MapPack};
use crate::connection::{Connection, Frame};
use crate::error::{Error, Result};
use crate::tables::{NOTABLES, Tables};
use crate::world::World;

/// Called with the live bot at the end of every successful `pump`.
/// The session worker installs one to publish heartbeat + route while a
/// script runs; `None` (default) keeps the SDK dependency-free and silent.
/// Must never pump: it runs with the bot already borrowed. Returning
/// false aborts the pump with `Error::Interrupted` (operator unload/close).
pub type PumpHook = Arc<dyn Fn(&mut Bot) -> bool + Send + Sync>;

/// Inline-pause gate, run at the end of every successful `pump` after the
/// hook. The session worker installs one: when a random event is pending
/// it runs the mapped rescue on this same thread and bot, then the caller
/// loop resumes where it was. Returning false aborts the pump with
/// `Error::Interrupted` (operator unload/close). `None` by default.
pub type PauseGate = Arc<dyn Fn(&mut Bot) -> bool + Send + Sync>;

pub use snapshot_view::Snapshot;

const KEEPALIVE_TICKS: u64 = 50;
/// Wall-clock twin of the tick keepalive: tick intake can stall behind a
/// slow drain, and the server counts silence in its own ticks. Whichever
/// fires first keeps the session alive.
const KEEPALIVE_WALL: Duration = Duration::from_secs(3);
const DRAIN_PER_PUMP: usize = 256;
/// Trail marks kept for the failure dossier (one per moved/ticked pump).
const TRAIL_CAP: usize = 256;

/// High-level bot: login, pump ticks, act, read live state.
pub struct Bot {
    conn: Connection,
    /// Live state, fed by every pumped frame. Scripts read it directly.
    pub world: World,
    /// Last intention shown by the orchestrator (`intend`).
    pub intent: String,
    pub username: String,
    host: String,
    port: u16,
    pack_dir: String,
    key_path: String,
    /// Last movement tick: public so offline tests can prime it (a bot
    /// that just sent needs no keepalive to plan purely).
    pub last_send_tick: u64,
    /// Last keepalive wall time: intake lag must not stretch the silence
    /// the server sees past its 100-tick timeout.
    pub last_keepalive: Option<Instant>,
    /// Current leg route, published for spectators.
    pub route: Vec<(i32, i32)>,
    pub route_target: Option<(i32, i32)>,
    /// Game-table lookups (names, sizes, component ids). `NOTABLES` by
    /// default: every lookup misses, NPC size falls back to 1.
    pub tables: Tables,
    /// Full collision map, painted square by square as the session
    /// explores (see `ensure_area`). Starts empty: unknown tiles read
    /// as blocked until their square loads. `travel_to` plans on the
    /// static world; this map carries live door states for execution.
    pub collision: CollisionMap,
    /// Openable blocking locs seen while painting, by tile. The router
    /// plans through them; the walker clicks the ones it must cross.
    /// Toll gates live here too but stay shut unless `allow_paid` is set.
    pub doors: HashMap<(i32, i32, i32), (u16, u8, u8)>,
    /// Tiles from `doors` whose opening charges a toll.
    pub toll_gates: HashSet<(i32, i32, i32)>,
    /// Doors already opened and verified live by the current `approach`
    /// call. A second encounter in the same call fails fast instead of
    /// re-clicking dynamic state. Cleared on entry of the outermost
    /// `approach`; nested door-stand approaches share it via
    /// `approach_depth`.
    pub approach_opened: HashSet<(i32, i32, i32)>,
    pub approach_depth: u32,
    /// Opt-in to paid paths (toll gates). Default false: routing and bank
    /// choice stay on the free side of every fence.
    pub allow_paid: bool,
    /// Opt-in to NPC shop purchases. Default true; a constrained task
    /// (`TaskSpec.allow_shop`) clears it and the buy paths refuse loud
    /// instead of spending, so the run reports blocked, never broke.
    pub allow_shop: bool,
    /// Opt-in to hunter zones (tree spots near aggressive NPCs). Default
    /// false: spots within range of something that would attack us are
    /// skipped.
    pub allow_wild: bool,
    /// Pack index for lazy map loading. `None` means no live painting:
    /// collision stays as streamed and `ensure_area` is a no-op.
    pub maps: Option<MapPack>,
    /// Static collision for the planner, wired by the session at open.
    /// `travel_to` refuses loud without it: no plan, no walk.
    pub nav_world: Option<Arc<lifescape_nav::static_world::StaticWorld>>,
    /// Planner tables (transitions, verdicts, door rules), wired by the
    /// session at open alongside `nav_world`.
    pub nav_tables: Option<Arc<lifescape_nav::tables::Tables>>,
    /// Pathfinding service handle (daemon IPC). `None` or a dead/refusing
    /// service means every plan fails loud with `nav-down`: never a local
    /// plan, the failure stays visible in the logs.
    pub plan_client: Option<lifescape_nav::service::PlanClient>,
    /// Data version sent with submits (set once at session open).
    pub plan_data_version: Option<String>,
    /// Served `wants` tags pinned by version: options build without
    /// tables and without a second round-trip while the version holds.
    pub(crate) wants_cache: Option<(String, Vec<String>)>,
    /// E14 walk-cost counters (remote calls vs in-batch duplicates).
    pub(crate) cost_stats: cost_cache::CostStats,
    /// Last positions with their tick, oldest first, capped
    /// (`TRAIL_CAP`): the failure dossier ships the last 50.
    pub trail: Vec<(u64, i32, i32, i32)>,
    /// Heartbeat hook, installed by the session worker (default: none).
    pub on_pump: Option<PumpHook>,
    /// Inline-pause gate, installed by the session worker (default: none).
    pub pause_gate: Option<PauseGate>,
    /// Pause ticks already billed to market pauses (and only those): the
    /// engagement budget counts work ticks, so the cycle subtracts this
    /// counter from the wall-clock tick delta. Advanced by the session
    /// pause gate around the inline market run, never by scripts.
    pub pause_ticks: u64,
    /// Zone label for log stamps (`zones::zone_name`), refreshed by the
    /// pump hook; "?" when unknown. Never a silent fallback: the "?" shows.
    pub zone_label: String,
}

impl Bot {
    pub fn new(host: &str, port: u16, pack_dir: &str, key_path: &str) -> Bot {
        Bot {
            conn: Connection::new(Duration::from_secs(10)),
            world: World::new(),
            intent: String::new(),
            username: String::new(),
            host: host.to_string(),
            port,
            pack_dir: pack_dir.to_string(),
            key_path: key_path.to_string(),
            last_send_tick: 0,
            last_keepalive: None,
            route: Vec::new(),
            route_target: None,
            tables: NOTABLES,
            collision: CollisionMap::new(),
            doors: HashMap::new(),
            toll_gates: HashSet::new(),
            approach_opened: HashSet::new(),
            approach_depth: 0,
            allow_paid: false,
            allow_shop: true,
            allow_wild: false,
            maps: None,
            nav_world: None,
            nav_tables: None,
            plan_client: None,
            plan_data_version: None,
            wants_cache: None,
            cost_stats: cost_cache::CostStats::default(),
            trail: Vec::new(),
            on_pump: None,
            pause_gate: None,
            pause_ticks: 0,
            zone_label: "?".to_string(),
        }
    }

    /// Attach live game tables (names, sizes, component ids). The SDK
    /// stays usable without them; lookups just miss.
    pub fn with_tables(mut self, tables: Tables) -> Bot {
        self.tables = tables;
        self
    }

    pub fn position(&self) -> (i32, i32, i32) {
        self.world.position()
    }

    /// Positions kept for the failure dossier: one mark per pump that
    /// moved or ticked, oldest first. Caps at `TRAIL_CAP`.
    pub(crate) fn note_trail(&mut self) {
        let mark = (self.world.ticks, self.world.x, self.world.z, self.world.level);
        if self.trail.last().is_some_and(|last| *last == mark) {
            return;
        }
        self.trail.push(mark);
        if self.trail.len() > TRAIL_CAP {
            let drop = self.trail.len() - TRAIL_CAP;
            self.trail.drain(..drop);
        }
    }

    /// Log in, retrying while the server still holds the previous session.
    /// Attempts and spacing are a process guard in seconds: ticks are
    /// frozen outside a connection, so no tick bound can apply here.
    /// The wait grows instead of hammering at a fixed period: the name
    /// frees when the old session logs out, and a fixed retry resonates
    /// with the 100-tick silence reap.
    pub fn login(&mut self, username: &str, password: Option<&str>) -> Result<()> {
        self.username = username.to_string();
        let password = password.unwrap_or(username).to_string();
        let mut last: Error = Error::Login("no attempt".to_string());
        for attempt in 0u32..15 {
            self.conn.close();
            self.conn.connect(&self.host, self.port)?;
            match self.conn.login(&self.username, &password, &self.pack_dir, &self.key_path, 274) {
                Ok(_) => {
                    // Fresh live state. The worker holds no static map (the
                    // service owns it); kept locs only matter to offline
                    // and direct tools that paint locally.
                    let static_locs = std::mem::take(&mut self.world.static_locs);
                    self.world = World::new();
                    self.world.static_locs = static_locs;
                    self.last_send_tick = 0;
                    self.last_keepalive = None;
                    self.drain_startup()?;
                    self.intend("connected");
                    return Ok(());
                }
                Err(Error::Login(message)) if message.contains("already logged in") => {
                    last = Error::Login(message);
                    std::thread::sleep(Self::relogin_backoff(attempt));
                }
                Err(other) => return Err(other),
            }
        }
        Err(last)
    }

    /// Reconnect after a dropped connection (bounded, process-guard sleep).
    pub fn relogin(&mut self) -> Result<()> {
        self.conn.close();
        self.login(&self.username.clone(), None)
    }

    /// Wait before re-probing a held name: 2 s, 4 s, then 8 s capped.
    /// Pure so the growth is unit-testable without a live server.
    fn relogin_backoff(attempt: u32) -> Duration {
        Duration::from_secs(2 << attempt.min(2))
    }

    fn drain_startup(&mut self) -> Result<()> {
        for _ in 0..500 {
            for frame in self.pump(1)? {
                if frame.name == "REBUILD_NORMAL" {
                    let _ = frame;
                }
                if self.world.ticks > 0 {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Record one finished market pause: work-tick accounting for the
    /// engagement budget. Session gate only.
    pub fn note_pause(&mut self, ticks: u64) {
        self.pause_ticks = self.pause_ticks.saturating_add(ticks);
    }

    /// Record what the bot is doing right now (dashboard state).
    pub fn intend(&mut self, intention: &str) {
        self.intent = intention.to_string();
    }

    /// One-line action trace (session log): every game action the bot
    /// sends, with wall clock and game tick. Carries its own stamp:
    /// script `println!` bypasses the worker stamp, so the worker
    /// cannot prefix it (the old comment promised it could).
    /// Read-only for planning: never call this from a planning view.
    pub fn act(&self, text: String) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|span| span.as_secs())
            .unwrap_or(0);
        println!(
            "[{:02}:{:02}:{:02}] act {} {text}",
            (secs / 3600) % 24,
            (secs / 60) % 60,
            secs % 60,
            self.tag()
        );
    }

    /// Script log line with wall clock and the exact game tick. The
    /// script holds the bot, so the tick needs no lookup.
    pub fn log(&self, text: &str) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|span| span.as_secs())
            .unwrap_or(0);
        println!(
            "[{:02}:{:02}:{:02}] {} {text}",
            (secs / 3600) % 24,
            (secs / 60) % 60,
            secs % 60,
            self.tag()
        );
    }

    /// Tick + zone stamp shared by every ticked log line: `[t=<tick> <zone>]`,
    /// "?" when unknown. Single source of format for SDK-side lines.
    pub fn tag(&self) -> String {
        format!("[t={} {}]", self.world.ticks, self.zone_label)
    }

    /// Pump `count` ticks: recv frames, feed the world, keep the session
    /// alive. Every game wait loops on this, never on wall-clock sleep.
    pub fn pump(&mut self, count: u32) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        for _ in 0..count {
            self.keepalive_if_due()?;
            let frame = self.conn.recv()?;
            self.world.feed(&frame.name, &frame.body)?;
            frames.push(frame);
            for _ in 0..DRAIN_PER_PUMP {
                if !self.conn.available()? {
                    break;
                }
                self.keepalive_if_due()?;
                let frame = self.conn.recv()?;
                self.world.feed(&frame.name, &frame.body)?;
                frames.push(frame);
            }
        }
        if let Some(hook) = self.on_pump.clone() {
            if !hook(self) {
                return Err(Error::Interrupted(format!("run stopped at {:?}", self.position())));
            }
        }
        self.note_trail();
        if let Some(gate) = self.pause_gate.clone() {
            if !gate(self) {
                return Err(Error::Interrupted(format!("run stopped at {:?}", self.position())));
            }
        }
        Ok(frames)
    }

    pub fn combat_level(&self) -> Option<u8> {
        crate::snapshot::combat_level(&self.world.stats)
    }

    pub fn want_run(&self) -> bool {
        self.world.energy >= 100
    }

    pub fn say(&mut self, text: &str) -> Result<()> {
        self.act(format!("say {text}"));
        let mut body = crate::buffer::Writer::new();
        body.u8(0);
        body.u8(0);
        body.raw(&crate::words::pack(text));
        self.send_named("MESSAGE_PUBLIC", &body.bytes())
    }

    /// NO_TIMEOUT keepalive (never IDLE_TIMER: the server treats that as
    /// an idle-logout request). Counts as a response, nothing more.
    pub fn idle(&mut self) -> Result<()> {
        self.send_named("NO_TIMEOUT", &[])
    }

    /// Keepalive when due by ticks or wall clock, whichever comes first.
    /// The wall leg covers intake stalls: a client that drains slower than
    /// the server still looks alive to the 100-tick silence timeout.
    fn keepalive_if_due(&mut self) -> Result<()> {
        let now = Instant::now();
        let wall_due = match self.last_keepalive {
            Some(seen) => now.duration_since(seen) >= KEEPALIVE_WALL,
            None => true,
        };
        if wall_due || self.world.ticks.saturating_sub(self.last_send_tick) >= KEEPALIVE_TICKS {
            self.idle()?;
            self.last_keepalive = Some(now);
        }
        Ok(())
    }

    fn send_named(&mut self, name: &str, payload: &[u8]) -> Result<()> {
        let outcome = self.conn.send(name, payload);
        if outcome.is_ok() {
            self.last_send_tick = self.world.ticks;
        }
        outcome
    }

    /// Unpaced send: door crossing only (travel.rs `step_door_through`).
    /// Every other caller uses the paced `send_named`.
    pub(crate) fn send_named_now(&mut self, name: &str, payload: &[u8]) -> Result<()> {
        let outcome = self.conn.send_immediate(name, payload);
        if outcome.is_ok() {
            self.last_send_tick = self.world.ticks;
        }
        outcome
    }

    /// Paint every map square touching the radius around (x, z).
    /// No pack index means no maps: nothing is painted.
    pub fn ensure_area(&mut self, x: i32, z: i32, radius: i32) {
        if let Some(pack) = self.maps.as_mut() {
            pack.ensure_area(&mut self.collision, &mut self.world.static_locs, &mut self.doors, &mut self.toll_gates, x, z, radius);
        }
    }

    /// Paint the full index (explicit full-map tools only).
    pub fn ensure_all(&mut self) {
        if let Some(pack) = self.maps.as_mut() {
            pack.ensure_all(&mut self.collision, &mut self.world.static_locs, &mut self.doors, &mut self.toll_gates);
        }
    }

    /// Copy of the live state for read-only serving off the pump path.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            position: self.world.position(),
            ticks: self.world.ticks,
            modal: self.world.modal.clone(),
            stats: self.world.stats.clone(),
            inv: self.world.inv.clone(),
            inv_cap: self.world.inv_cap.clone(),
            backpack_com: (self.tables.com_id)("inventory:inv"),
            texts: self.world.texts.clone(),
            energy: self.world.energy,
            intent: self.intent.clone(),
        }
    }

    /// Best-effort logout trigger, then the socket shuts anyway. A missed
    /// trigger only costs a server-side idle timeout: the next login
    /// retries through "already logged in".
    pub fn close(&mut self) {
        if let Some(com) = (self.tables.com_id)("logout:try_logout") {
            let mut body = crate::buffer::Writer::new();
            body.u16(com);
            let _ = self.send_named("IF_BUTTON", &body.bytes());
        }
        self.conn.close();
        self.intent = "closed".to_string();
    }
}

/// Aggregate carried items: {obj_id: count} across open inventories.
pub fn aggregate(inv: &HashMap<u16, HashMap<u8, (u16, u32)>>) -> HashMap<u16, u32> {
    crate::snapshot::aggregate(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_stamps_tick_and_zone() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        assert_eq!(bot.tag(), "[t=0 ?]");
        bot.world.ticks = 447;
        bot.zone_label = "Lumbridge".to_string();
        assert_eq!(bot.tag(), "[t=447 Lumbridge]");
    }

    /// Held-name probe backs off (2 s, 4 s, 8 s capped) instead of
    /// hammering at a fixed period.
    #[test]
    fn relogin_backoff_grows_then_caps() {
        let waits: Vec<u64> = (0..8).map(|attempt| Bot::relogin_backoff(attempt).as_secs()).collect();
        assert_eq!(waits, vec![2, 4, 8, 8, 8, 8, 8, 8]);
    }
}
