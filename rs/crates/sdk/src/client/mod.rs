//! Bot core: connection lifecycle, tick pump and live snapshots.
//!
//! Movement and actions live in sibling modules; this module owns the
//! session (login, pump, snapshot, close).

pub mod actions;
pub mod approach;
pub mod cost_cache;
pub mod design;
pub mod doors;
pub mod exchange;
pub mod loc_stands;
pub mod locks;
pub mod movement;
pub mod nav_remote;
pub mod service_wait;
pub mod snapshot_view;
pub mod social;
pub mod travel;
pub mod travel_custom;
pub mod travel_dossier;
pub mod travel_hook;

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
/// Frame bound of one `catch_up`: a 45-tick backlog is a few hundred
/// frames; the bound only guards a server that never stops talking.
const CATCH_UP_CAP: usize = 4096;
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
    /// Ground objects the server refused to hand over this session
    /// (`(obj, x, z, level)`, "I can't reach that!" from every side):
    /// `take` skips them instead of re-clicking after every kill (live
    /// 2026-11-12: an egg past the Port Sarim pen fence, ~340 refused
    /// clicks in 10 min). Memory only, gone with the session.
    pub unreachable_objs: HashSet<(u16, i32, i32, i32)>,
    /// Market locks pushed by the daemon (see `locks`): bank withdrawals
    /// never take the part promised to open orders, outside a trade.
    pub locks: locks::SharedLocks,
    /// Opt-in to paid paths (toll gates). Default false: routing and bank
    /// choice stay on the free side of every fence.
    pub allow_paid: bool,
    /// Coins a paid run keeps in the pack (the fares are taken from the
    /// inventory): 10 for the Al Kharid toll, 60 for a Karamja round trip
    /// (30 each way). 0 outside paid runs; bank trips refill to it.
    pub fare_reserve: u32,
    /// Opt-in to NPC shop purchases. Default true; a constrained task
    /// (`TaskSpec.allow_shop`) clears it and the buy paths refuse loud
    /// instead of spending, so the run reports blocked, never broke.
    pub allow_shop: bool,
    /// Combat style the running script trains (`melee`, `ranged`,
    /// `magic`): the gear kit scores bows/arrows or staves for it and the
    /// fight loop casts instead of clicking attack. Script state, never
    /// sent anywhere.
    pub combat_style: String,
    /// Spell a magic run casts (`--spell`, the planner sized its runes):
    /// `None` = the strongest damage spell the pack funds. Script state,
    /// set by every magic run, never sent anywhere.
    pub combat_spell: Option<String>,
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
    /// Danger squares for this bot's combat level (scripts arm them from
    /// gamedata, `kit::danger::arm`): every plan request carries them so
    /// routes avoid hunters (`NavOptions::danger`).
    pub nav_danger: Vec<lifescape_nav::plan::DangerZone>,
    /// Site the brain assigned to this run (x, z, level): gather pickers
    /// only consider resources around it (`--site`, kit::cycle).
    pub assigned_site: Option<(i32, i32, i32)>,
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
    /// Start tick of the market pause in flight (`begin_pause`), cleared
    /// by `note_pause`: the budget hook bills the running pause as pause,
    /// never as work (live 2026-11-13: a 1500-tick fill run killed its
    /// own inline delivery at the square, deal `timeout`).
    pub pause_since: Option<u64>,
    /// Market pauses finished so far (`note_pause`): a trip whose plan
    /// spans one replans from where the pause left the bot.
    pub pauses_done: u64,
    /// Zone label for log stamps (`zones::zone_name`), refreshed by the
    /// pump hook; "?" when unknown. Never a silent fallback: the "?" shows.
    pub zone_label: String,
    /// Auto-run latch: set when energy hits full, held while it drains,
    /// released at empty. Two thresholds so the run flag never flaps
    /// around one (walk legs read it through `want_run`).
    pub run_latch: bool,
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
            unreachable_objs: HashSet::new(),
            locks: std::sync::Arc::new(std::sync::Mutex::new(locks::Locks::default())),
            approach_depth: 0,
            allow_paid: false,
            fare_reserve: 0,
            allow_shop: true,
            combat_style: "melee".to_string(),
            combat_spell: None,
            allow_wild: false,
            maps: None,
            nav_world: None,
            nav_tables: None,
            plan_client: None,
            nav_danger: Vec::new(),
            assigned_site: None,
            plan_data_version: None,
            wants_cache: None,
            cost_stats: cost_cache::CostStats::default(),
            trail: Vec::new(),
            on_pump: None,
            pause_gate: None,
            pause_ticks: 0,
            pause_since: None,
            pauses_done: 0,
            zone_label: "?".to_string(),
            run_latch: false,
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
                    // The last known bank survives a relogin (only a bank
                    // visit changes it); a first login seeds it from the
                    // account save the server just loaded.
                    let bank_com = (self.tables.com_id)("bank_main:inv");
                    let mut closed = std::mem::take(&mut self.world.inv_closed);
                    if let Some(open_bank) = bank_com.and_then(|com| self.world.inv.remove(&com).map(|items| (com, items))) {
                        closed.insert(open_bank.0, open_bank.1);
                    }
                    closed.retain(|com, _| Some(*com) == bank_com);
                    self.world = World::new();
                    self.world.static_locs = static_locs;
                    self.world.inv_closed = closed;
                    self.seed_bank_from_save();
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

    /// Seeds the last known bank from the account save when this session
    /// never saw it: the server loads that file at login, so it is the
    /// truth until the first bank visit retransmits. A missing save (other
    /// server dir, fresh account) leaves the bank unknown, as before.
    fn seed_bank_from_save(&mut self) {
        let Some(bank_com) = (self.tables.com_id)("bank_main:inv") else { return };
        if self.world.inv_known(bank_com).is_some() {
            return;
        }
        if let Some(save) = crate::save::load_account(&self.username, None) {
            self.world.seed_closed_inv(bank_com, &save.bank);
        }
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
        self.pause_since = None;
        self.pauses_done = self.pauses_done.saturating_add(1);
    }

    /// Open one market pause at the current tick (session gate only);
    /// `note_pause` closes it.
    pub fn begin_pause(&mut self) {
        self.pause_since = Some(self.world.ticks);
    }

    /// Pause ticks billed so far, the pause in flight included: what the
    /// engagement budget subtracts from the wall-clock delta.
    pub fn pause_ticks_now(&self) -> u64 {
        let running = self.pause_since.map(|since| self.world.ticks.saturating_sub(since)).unwrap_or(0);
        self.pause_ticks.saturating_add(running)
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

    /// Feed one pumped frame, acking rebuilds like the client: after
    /// `mapBuild` it sends `MAP_BUILD_COMPLETE` (`Client.ts:5137`), and
    /// the server streams the new region only once the ack lands.
    fn feed(&mut self, frame: &Frame) -> Result<()> {
        self.world.feed(&frame.name, &frame.body)?;
        if frame.name == "REBUILD_NORMAL" {
            self.send_named("MAP_BUILD_COMPLETE", &[])?;
        }
        Ok(())
    }

    /// Pump `count` ticks: recv frames, feed the world, keep the session
    /// alive. Every game wait loops on this, never on wall-clock sleep.
    pub fn pump(&mut self, count: u32) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        for _ in 0..count {
            self.keepalive_if_due()?;
            let frame = self.conn.recv()?;
            self.feed(&frame)?;
            frames.push(frame);
            for _ in 0..DRAIN_PER_PUMP {
                if !self.conn.available()? {
                    break;
                }
                self.keepalive_if_due()?;
                let frame = self.conn.recv()?;
                self.feed(&frame)?;
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

    /// Read every frame the server already sent, never blocking. After
    /// a stretch without pumping (a slow plan answer: 0.2-2.7 s live,
    /// i.e. 3-45 dev ticks) the socket holds ticks that predate the next
    /// send; a tick watch started before reading them counts them as
    /// its own. Returns the ticks drained. No hooks: the next pump runs
    /// them.
    pub(crate) fn catch_up(&mut self) -> Result<u64> {
        let before = self.world.ticks;
        let mut drained = 0usize;
        while drained < CATCH_UP_CAP && self.conn.available()? {
            self.keepalive_if_due()?;
            let frame = self.conn.recv()?;
            self.feed(&frame)?;
            drained += 1;
        }
        if drained > 0 {
            self.note_trail();
        }
        Ok(self.world.ticks.saturating_sub(before))
    }

    pub fn combat_level(&self) -> Option<u8> {
        crate::snapshot::combat_level(&self.world.stats)
    }

    /// Auto-run for every walk leg: latches on at full energy, stays on
    /// while it drains, releases at empty. Mirrors the run button (the
    /// server forces run off at 0 too); MOVE legs just carry the flag.
    pub fn want_run(&mut self) -> bool {
        if self.world.energy >= 100 {
            self.run_latch = true;
        } else if self.world.energy == 0 {
            self.run_latch = false;
        }
        self.run_latch
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
        // Tells cap with the heartbeat (matches the 25 chat lines): the
        // live log is unbounded, the snapshot is per-second traffic.
        let mut private: Vec<crate::world::ChatLine> = self.world.private.iter().rev().take(25).cloned().collect();
        private.reverse();
        Snapshot {
            position: self.world.position(),
            ticks: self.world.ticks,
            modal: self.world.modal.clone(),
            stats: self.world.stats.clone(),
            inv: self.world.inv.clone(),
            inv_cap: self.world.inv_cap.clone(),
            backpack_com: (self.tables.com_id)("inventory:inv"),
            texts: self.world.texts.clone(),
            private,
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

    /// The market pause in flight counts as pause from its first tick;
    /// `note_pause` closes it into the billed total.
    #[test]
    fn pause_in_flight_is_billed_as_pause() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.ticks = 1000;
        bot.note_pause(40);
        assert_eq!(bot.pause_ticks_now(), 40, "closed pauses only");
        bot.begin_pause();
        bot.world.ticks = 1700;
        assert_eq!(bot.pause_ticks_now(), 740, "the running pause counts too");
        bot.note_pause(700);
        assert_eq!(bot.pause_since, None, "closed");
        bot.world.ticks = 1800;
        assert_eq!(bot.pause_ticks_now(), 740, "no double count once closed");
    }

    #[test]
    fn tag_stamps_tick_and_zone() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        assert_eq!(bot.tag(), "[t=0 ?]");
        bot.world.ticks = 447;
        bot.zone_label = "Lumbridge".to_string();
        assert_eq!(bot.tag(), "[t=447 Lumbridge]");
    }

    /// Auto-run latches at full, holds while draining, releases at
    /// empty, stays off through the regen climb: no flap, one place
    /// (every walk leg reads `want_run`).
    #[test]
    fn run_latch_holds_until_empty() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.energy = 100;
        assert!(bot.want_run(), "full energy latches run on");
        assert!(bot.want_run(), "already on stays on, no re-toggle");
        bot.world.energy = 93;
        assert!(bot.want_run(), "draining holds the latch");
        bot.world.energy = 1;
        assert!(bot.want_run(), "near-empty still holds");
        bot.world.energy = 0;
        assert!(!bot.want_run(), "empty releases the latch");
        bot.world.energy = 50;
        assert!(!bot.want_run(), "regen climb stays off until full");
        bot.world.energy = 100;
        assert!(bot.want_run(), "full latches back on");
    }

    /// Received tells are observable off the pump path: a fed
    /// `MESSAGE_PRIVATE` lands in the snapshot the calls serve.
    #[test]
    fn received_tells_reach_the_snapshot() {
        use crate::words::{pack, to_base37, unpack};
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        let mut body = to_base37("t02").to_be_bytes().to_vec();
        body.extend([0, 0, 0, 0, 0]);
        body.extend(pack("meet at varrock"));
        bot.world.feed("MESSAGE_PRIVATE", &body).expect("tell feeds");
        assert_eq!(bot.world.private.len(), 1, "live log records the tell");
        let snap = bot.snapshot();
        assert_eq!(snap.private.len(), 1, "snapshot carries the tell");
        assert_eq!(snap.private[0].who, "t02");
        // Wire packing capitalizes and pads: the snapshot carries the
        // decoded text, whatever the packing normalized.
        assert_eq!(snap.private[0].text, unpack(&pack("meet at varrock")));
    }

    /// Held-name probe backs off (2 s, 4 s, 8 s capped) instead of
    /// hammering at a fixed period.
    #[test]
    fn relogin_backoff_grows_then_caps() {
        let waits: Vec<u64> = (0..8).map(|attempt| Bot::relogin_backoff(attempt).as_secs()).collect();
        assert_eq!(waits, vec![2, 4, 8, 8, 8, 8, 8, 8]);
    }

    /// A fed rebuild updates the zone first, then attempts the ack: the
    /// dead socket fails at `Login`, proving the send was reached (the
    /// client sends `MAP_BUILD_COMPLETE` after every rebuild). A plain
    /// frame feeds clean offline, proving only rebuilds ack.
    #[test]
    fn rebuild_feeds_state_then_acks_like_the_client() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        let rebuild = Frame { opcode: 231, name: "REBUILD_NORMAL".to_string(), body: vec![0, 50, 0, 50] };
        let err = bot.feed(&rebuild).expect_err("ack send needs a socket");
        assert!(format!("{err}").contains("not logged in"), "ack attempted after feed, got: {err}");
        assert_eq!((bot.world.base_x, bot.world.base_z), ((50 - 6) << 3, (50 - 6) << 3), "feed ran before the ack");
        let plain = Frame { opcode: 171, name: "IF_CLOSE".to_string(), body: vec![] };
        assert!(bot.feed(&plain).is_ok(), "non-rebuild sends nothing");
    }

    /// The ack resolves its op name (a bad name fails `Protocol`, never
    /// reaches the socket): dead socket reads `Login`, proving the send
    /// path is wired for the empty payload.
    #[test]
    fn map_build_complete_reaches_the_send_path() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        let outcome = bot.send_named("MAP_BUILD_COMPLETE", &[]);
        assert!(matches!(outcome, Err(Error::Login(_))), "dead socket fails at login, never at the op name: {outcome:?}");
    }
}
