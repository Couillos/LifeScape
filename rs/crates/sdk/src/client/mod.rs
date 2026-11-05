//! Bot core: connection lifecycle, tick pump and live snapshots.
//!
//! Movement and actions live in sibling modules; this module owns the
//! session (login, pump, snapshot, close).

pub mod actions;
pub mod movement;
pub mod snapshot_view;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::collision::{CollisionMap, MapPack};
use crate::client::movement::TransitionRow;
use crate::connection::{Connection, Frame};
use crate::error::{Error, Result};
use crate::tables::{NOTABLES, Tables};
use crate::world::World;

/// Called with the live bot at the end of every successful `pump`.
/// The session worker installs one to publish heartbeat + route while a
/// script runs; `None` (default) keeps the SDK dependency-free and silent.
/// Must never pump: it runs with the bot already borrowed. Returning
/// false aborts the pump with `Error::Interrupted` (operator unload/close).
pub type PumpHook = Arc<dyn Fn(&Bot) -> bool + Send + Sync>;

pub use snapshot_view::Snapshot;

const KEEPALIVE_TICKS: u64 = 50;
const DRAIN_PER_PUMP: usize = 256;

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
    last_send_tick: u64,
    /// Current leg route, published for spectators.
    pub route: Vec<(i32, i32)>,
    pub route_target: Option<(i32, i32)>,
    /// Game-table lookups (names, sizes, component ids). `NOTABLES` by
    /// default: every lookup misses, NPC size falls back to 1.
    pub tables: Tables,
    /// Full collision map when the session loaded the pack maps; `walk_to`
    /// chains BFS windows through it, otherwise it walks straight legs.
    /// Full collision map, painted square by square as the session
    /// explores (see `ensure_area`). Starts empty: unknown tiles read
    /// as blocked until their square loads.
    pub collision: CollisionMap,
    /// Openable blocking locs seen while painting, by tile. The router
    /// plans through them; the walker clicks the ones it must cross.
    /// Toll gates live here too but stay shut unless `allow_paid` is set.
    pub doors: HashMap<(i32, i32, i32), (u16, u8, u8)>,
    /// Tiles from `doors` whose opening charges a toll.
    pub toll_gates: HashSet<(i32, i32, i32)>,
    /// Opt-in to paid paths (toll gates). Default false: routing and bank
    /// choice stay on the free side of every fence.
    pub allow_paid: bool,
    /// Pack index for lazy map loading. `None` means straight legs only.
    pub maps: Option<MapPack>,
    /// Known stair/ladder/trapdoor rides, loaded from the transition dump.
    /// Empty without it: climbs fall back to the nearest ladder loc.
    pub transitions: Vec<TransitionRow>,
    /// Heartbeat hook, installed by the session worker (default: none).
    pub on_pump: Option<PumpHook>,
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
            route: Vec::new(),
            route_target: None,
            tables: NOTABLES,
            collision: CollisionMap::new(),
            doors: HashMap::new(),
            toll_gates: HashSet::new(),
            allow_paid: false,
            maps: None,
            transitions: Vec::new(),
            on_pump: None,
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

    /// Log in, retrying while the server still holds the previous session.
    /// Attempts and spacing are a process guard in seconds: ticks are
    /// frozen outside a connection, so no tick bound can apply here.
    pub fn login(&mut self, username: &str, password: Option<&str>) -> Result<()> {
        self.username = username.to_string();
        let password = password.unwrap_or(username).to_string();
        let mut last: Error = Error::Login("no attempt".to_string());
        for _ in 0..15 {
            self.conn.close();
            self.conn.connect(&self.host, self.port)?;
            match self.conn.login(&self.username, &password, &self.pack_dir, &self.key_path, 274) {
                Ok(_) => {
                    // Fresh live state, but keep the loaded static map:
                    // sessions reload it once at open, not per login.
                    let static_locs = std::mem::take(&mut self.world.static_locs);
                    self.world = World::new();
                    self.world.static_locs = static_locs;
                    self.last_send_tick = 0;
                    self.drain_startup()?;
                    self.intend("connected");
                    return Ok(());
                }
                Err(Error::Login(message)) if message.contains("already logged in") => {
                    last = Error::Login(message);
                    std::thread::sleep(Duration::from_secs(2));
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

    /// Record what the bot is doing right now (dashboard state).
    pub fn intend(&mut self, intention: &str) {
        self.intent = intention.to_string();
    }

    /// Pump `count` ticks: recv frames, feed the world, keep the session
    /// alive. Every game wait loops on this, never on wall-clock sleep.
    pub fn pump(&mut self, count: u32) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        for _ in 0..count {
            if self.world.ticks.saturating_sub(self.last_send_tick) >= KEEPALIVE_TICKS {
                self.idle()?;
            }
            let frame = self.conn.recv()?;
            self.world.feed(&frame.name, &frame.body)?;
            frames.push(frame);
            for _ in 0..DRAIN_PER_PUMP {
                if !self.conn.available()? {
                    break;
                }
                if self.world.ticks.saturating_sub(self.last_send_tick) >= KEEPALIVE_TICKS {
                    self.idle()?;
                }
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
        Ok(frames)
    }

    pub fn combat_level(&self) -> Option<u8> {
        crate::snapshot::combat_level(&self.world.stats)
    }

    pub fn want_run(&self) -> bool {
        self.world.energy >= 100
    }

    pub fn say(&mut self, text: &str) -> Result<()> {
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

    fn send_named(&mut self, name: &str, payload: &[u8]) -> Result<()> {
        let outcome = self.conn.send(name, payload);
        if outcome.is_ok() {
            self.last_send_tick = self.world.ticks;
        }
        outcome
    }

    /// Paint every map square touching the radius around (x, z).
    /// No pack index means no maps: straight legs only, everywhere.
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
