#![forbid(unsafe_code)]
//! Inline rescue pause: worker-side detection and transparent execution.
//!
//! The worker watches for unresolved random events for the whole session
//! life (idle and running) and poses `pending_rescue` once, with a push
//! frame to the daemon plus one human log line as telemetry. The script
//! thread (through the SDK pause gate) or the idle pump (when no script
//! runs) then runs the mapped rescue inline, and the script loop resumes
//! where it was: pause, never re-run. `rescue_gate` allows a single level
//! (pumps inside the rescue never re-trigger); `rescue_done` stops repeats
//! at the same spot. The user pause (`inner.paused`) is untouched: rescue
//! uses its own flags.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use lifescape_sdk::client::Snapshot;
use lifescape_sdk::Bot;
use lifescape_scripts::randomevent::{self, EventKind};

use lifescape_json::{object, str_value, Value};
use crate::session::{now_secs, script_error_type, Shared};

/// One posed rescue: the registry script name, where it was seen, and the
/// tick it was posed (Defaut 4 budget start). Same (rescue, position)
/// repeats are dropped, a new key replaces the old one.
pub(crate) struct PendingRescue {
    pub(crate) rescue: String,
    pub(crate) pos: (i32, i32, i32),
    pub(crate) started: u64,
}

/// Defaut 4, rescue borne: a rescue that never clears (tower fail spot,
/// olivia_king > 90 min) stops consuming the worker. 1800 ticks ~= 108 s
/// en dev, 18 min en prod: 30x un climb normal, bien sous les 9000 ticks
/// du cas observe.
pub const RESCUE_BUDGET_TICKS: u64 = 1800;

/// Budget check on game ticks (saturating: tick resets never fake an expiry).
pub fn rescue_expired(started: u64, now: u64) -> bool {
    now.saturating_sub(started) >= RESCUE_BUDGET_TICKS
}

/// Dossier path for an expired rescue (hors-ligne, rejouable comme les
/// dossiers nav): `run/failures/<compte>-rescue-<tick>.json`.
pub fn rescue_dossier_path(account: &str, tick: u64) -> std::path::PathBuf {
    crate::paths::run_dir().join("failures").join(format!("{account}-rescue-{tick}.json"))
}

/// Expire one rescue loudly: dossier on disk, typed `RescueBudget` in
/// `last_error`, repeat guard set so the observer does not re-pose the
/// same spot. The main script resumes (pause, never re-run) and its own
/// report drives the brain replan; idle workers simply stop looping.
pub(crate) fn fail_rescue_budget(shared: &Shared, pending: &PendingRescue, now: u64) {
    let dossier = object(vec![
        ("account", str_value(&shared.account)),
        ("rescue", str_value(&pending.rescue)),
        (
            "pos",
            Value::Arr(vec![
                Value::Int(i64::from(pending.pos.0)),
                Value::Int(i64::from(pending.pos.1)),
                Value::Int(i64::from(pending.pos.2)),
            ]),
        ),
        ("started", Value::Int(pending.started as i64)),
        ("tick", Value::Int(now as i64)),
        ("budget_ticks", Value::Int(RESCUE_BUDGET_TICKS as i64)),
        ("stop", str_value("rescue_budget")),
    ]);
    let path = rescue_dossier_path(&shared.account, now);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, lifescape_json::stringify(&dossier));
    let script_args = shared.inner.lock().unwrap().script_args.clone();
    crate::session_push::push_event(
        &shared.account,
        "rescue_budget",
        &pending.rescue,
        false,
        now,
        &pending.rescue,
        &script_args,
        &format!("rescue {} over budget ({} ticks)", pending.rescue, RESCUE_BUDGET_TICKS),
        pending.pos,
    );
    *shared.rescue_done.lock().unwrap() = Some((pending.rescue.clone(), pending.pos));
    shared.inner.lock().unwrap().last_error = Some(object(vec![
        ("where", str_value(&pending.rescue)),
        ("type", str_value("RescueBudget")),
        (
            "message",
            str_value(&format!("rescue {} over budget at {:?}", pending.rescue, pending.pos)),
        ),
    ]));
    shared.log(&format!("rescue {} over budget at {:?}, dossier {}", pending.rescue, pending.pos, path.display()));
}

/// Live-bot detection: maze, mime, carried cube, fail-teleport landing,
/// visible event host (mage, genie, dwarf — greeted, never zoned).
pub(crate) fn detect_live(bot: &Bot) -> Option<EventKind> {
    if let Some(kind) = randomevent::event_at(bot) {
        return Some(kind);
    }
    if randomevent::in_fail_teleport(bot) {
        return Some(EventKind::Tower);
    }
    // Hosts have no zone: a streamed keeper of the event is the signal.
    // The adjacent-only `greet_near` cannot reach diagonal or two-tile
    // hosts, and an ignored dwarf escalates to stones — so pose the
    // walking `rescue_greet` for any visible host instead.
    if lifescape_scripts::randomevent_full::EVENT_HOSTS.iter().any(|name| bot.find_npc(name).is_some()) {
        return Some(EventKind::Host);
    }
    None
}

/// Snapshot detection for a running script the observer cannot borrow
/// (the script thread holds the bot for its whole run): same geography
/// from the heartbeat position, cube from the streamed pack. Hosts are
/// invisible here (snapshots carry no NPC list): running scripts greet
/// them through their own `check()` cadence instead.
pub(crate) fn detect_snapshot(snapshot: &Snapshot) -> Option<(EventKind, (i32, i32, i32))> {
    let (level, x, z) = snapshot.position;
    if randomevent::in_maze_at(x, z) {
        return Some((EventKind::Maze, (level, x, z)));
    }
    if randomevent::in_mime_at(x, z) {
        return Some((EventKind::Mime, (level, x, z)));
    }
    if randomevent::in_fail_teleport_at((level, x, z)) {
        return Some((EventKind::Tower, (level, x, z)));
    }
    if let Some(pack) = snapshot.backpack_com {
        if let Some(cube) = lifescape_gamedata::objs_by_name("macro_cube").into_iter().next() {
            let cube = cube.obj_id as u16;
            if snapshot.inv.get(&pack).is_some_and(|slots| slots.values().any(|(held, _)| *held == cube)) {
                return Some((EventKind::Cube, (level, x, z)));
            }
        }
    }
    None
}

/// Pose a rescue once: same (rescue, position) repeats are dropped, a new
/// key replaces the old one. Poses the shared flag for the pause gate AND
/// pushes a structured event frame to the daemon; the log keeps one human
/// line (`event <kind> resolved=false`), never a control marker.
pub(crate) fn pose(shared: &Shared, kind: EventKind, pos: (i32, i32, i32), tick: u64) {
    let rescue = randomevent::rescue_for(kind).to_string();
    {
        let mut slot = shared.pending_rescue.lock().unwrap();
        if let Some(existing) = slot.as_ref() {
            // Budget first: an expired pose fails loudly instead of
            // refreshing silently (olivia_king tower loop).
            if rescue_expired(existing.started, tick) {
                let expired = PendingRescue {
                    rescue: existing.rescue.clone(),
                    pos: existing.pos,
                    started: existing.started,
                };
                drop(slot);
                fail_rescue_budget(shared, &expired, tick);
                shared.pending_rescue.lock().unwrap().take();
                return;
            }
            if existing.rescue == rescue && existing.pos == pos {
                return;
            }
        }
        // Repeat guard: a budgeted rescue never re-poses at the same spot.
        if shared.rescue_done.lock().unwrap().as_ref().is_some_and(|done| done.0 == rescue && done.1 == pos) {
            return;
        }
        *slot = Some(PendingRescue { rescue, pos, started: tick });
    }
    let name = format!("{kind:?}").to_lowercase();
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.last_heartbeat_tick = tick;
        let mut entry = std::collections::BTreeMap::new();
        entry.insert("event".to_string(), str_value(&name));
        entry.insert("resolved".to_string(), Value::Bool(false));
        entry.insert("at".to_string(), Value::Float(now_secs()));
        inner.last_event = Some(Value::Obj(entry));
    }
    let rescue = randomevent::rescue_for(kind);
    let script_args = shared.inner.lock().unwrap().script_args.clone();
    crate::session_push::push_event(&shared.account, "random_event", &name, false, tick, rescue, &script_args, "", pos);
    shared.log(&format!("event {name} resolved=false"));
}

/// Drop a stale pose and its repeat guard: nothing pending, no reason to
/// skip a future rescue at the same spot.
fn clear_rescue_state(shared: &Shared) {
    shared.pending_rescue.lock().unwrap().take();
    shared.rescue_done.lock().unwrap().take();
}

/// One posed market delivery: the script the gate runs inline at the
/// next safe point, then the bot walks back to its work. Posed by the
/// brain (later lots) or the operator; consumed once, like a rescue.
pub(crate) struct PendingMarket {
    pub(crate) script: String,
    pub(crate) args: Vec<String>,
}

/// Pose a market delivery: same single-level gate as the rescue, lower
/// priority (the rescue branch runs first). Overwrites any earlier pose:
/// one convocation at a time.
pub(crate) fn pose_market(shared: &Shared, script: &str, args: &[String], tick: u64) {
    *shared.pending_market.lock().unwrap() =
        Some(PendingMarket { script: script.to_string(), args: args.to_vec() });
    shared.log(&format!("market {script} posed, waits for the safe point"));
    crate::session_push::push_event(&shared.account, "market", script, false, tick, script, args, "", (0, 0, 0));
}

/// Pause gate: runs at the end of every `pump` on the script thread. A
/// posed rescue with a free gate runs inline now (same thread, same bot)
/// and the script loop resumes after this returns. Otherwise a posed
/// market delivery runs at the next safe point (alive, no modal), with
/// the engagement clock suspended across it. False only when the
/// operator stopped the run.
pub(crate) fn gate_tick(shared: &Shared, bot: &mut Bot) -> bool {
    let stopped = || shared.stop_requested.load(Ordering::SeqCst);
    if shared.inner.lock().unwrap().held || shared.rescue_gate.swap(true, Ordering::SeqCst) {
        return !stopped();
    }
    let pending = shared.pending_rescue.lock().unwrap().take();
    let Some(pending) = pending else {
        return gate_market(shared, bot, stopped());
    };
    // Defaut 4: bounded inline rescue. Over budget, the pose fails
    // typed (dossier + RescueBudget) and the script resumes; its own
    // end report drives the brain replan, never another rescue loop.
    if rescue_expired(pending.started, bot.world.ticks) {
        fail_rescue_budget(shared, &pending, bot.world.ticks);
        shared.rescue_gate.store(false, Ordering::SeqCst);
        return !stopped();
    }
    if shared.rescue_done.lock().unwrap().as_ref().is_some_and(|done| done.0 == pending.rescue && done.1 == pending.pos) {
        *shared.pending_rescue.lock().unwrap() = Some(pending);
        shared.rescue_gate.store(false, Ordering::SeqCst);
        return !stopped();
    }
    bot.intend(&format!("pausing for {}", pending.rescue));
    let outcome = lifescape_scripts::run(&pending.rescue, bot, &[]);
    if stopped() || shared.inner.lock().unwrap().closing {
        // Operator unload/close: unwind, the observer re-poses if needed.
        shared.rescue_gate.store(false, Ordering::SeqCst);
        return false;
    }
    let end_pos = bot.position();
    *shared.rescue_done.lock().unwrap() = Some((pending.rescue.clone(), end_pos));
    match outcome {
        Ok(_) => {
            shared.log(&format!("rescue {} done at {end_pos:?}, script resumes", pending.rescue));
        }
        Err(error) => {
            let (level, pos_x, pos_z) = end_pos;
            let message = error.to_string();
            let script_args = shared.inner.lock().unwrap().script_args.clone();
            crate::session_push::push_event(
                &shared.account,
                "script_error",
                &pending.rescue,
                false,
                bot.world.ticks,
                &pending.rescue,
                &script_args,
                &message,
                end_pos,
            );
            shared.inner.lock().unwrap().last_heartbeat_tick = bot.world.ticks;
            shared.log(&format!("event script_error {} resolved=false", pending.rescue));
            shared.inner.lock().unwrap().last_error = Some(object(vec![
                ("where", str_value(&pending.rescue)),
                ("type", str_value(script_error_type(&error))),
                ("message", str_value(&message)),
                (
                    "at",
                    Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))]),
                ),
            ]));
        }
    }
    shared.rescue_gate.store(false, Ordering::SeqCst);
    true
}

/// Market branch of the pause gate: at the shared safe point only
/// (alive, no modal), the posed script runs inline and the bot walks
/// back to its work. The engagement clock is suspended across the whole
/// pause, return trip included; the ticks land in the task report.
fn gate_market(shared: &Shared, bot: &mut Bot, stopped: bool) -> bool {
    let market = shared.pending_market.lock().unwrap().take();
    let Some(market) = market else {
        shared.rescue_gate.store(false, Ordering::SeqCst);
        return !stopped;
    };
    if !lifescape_scripts::kit::survival::at_safe_point(bot) {
        *shared.pending_market.lock().unwrap() = Some(market);
        shared.rescue_gate.store(false, Ordering::SeqCst);
        return !stopped;
    }
    let return_to = bot.position();
    bot.intend(&format!("pausing for market {}", market.script));
    let pause_start = bot.world.ticks;
    let outcome = lifescape_scripts::run(&market.script, bot, &market.args);
    // Best effort walk back to the work tile: a failed return degrades
    // to resuming wherever the delivery left us, never to a re-run.
    let (level, pos_x, pos_z) = return_to;
    if let Err(error) = bot.travel_to(lifescape_sdk::plan::Tile { x: pos_x, z: pos_z, level }) {
        bot.log(&format!("market return walk failed at {:?}: {error}, resuming here", bot.position()));
    }
    let paused = bot.world.ticks.saturating_sub(pause_start);
    bot.note_pause(paused);
    {
        let mut total = shared.market_pause_ticks.lock().unwrap();
        *total = total.saturating_add(paused);
    }
    if shared.stop_requested.load(Ordering::SeqCst) || shared.inner.lock().unwrap().closing {
        shared.rescue_gate.store(false, Ordering::SeqCst);
        return false;
    }
    match outcome {
        Ok(_) => {
            shared.log(&format!("market {} done, script resumes (pause {paused} ticks)", market.script));
        }
        Err(error) => {
            let (end_level, end_x, end_z) = bot.position();
            let message = error.to_string();
            let script_args = shared.inner.lock().unwrap().script_args.clone();
            crate::session_push::push_event(
                &shared.account,
                "script_error",
                &market.script,
                false,
                bot.world.ticks,
                &market.script,
                &script_args,
                &message,
                (end_level, end_x, end_z),
            );
            shared.inner.lock().unwrap().last_heartbeat_tick = bot.world.ticks;
            shared.log(&format!("event script_error {} resolved=false", market.script));
        }
    }
    shared.rescue_gate.store(false, Ordering::SeqCst);
    true
}

/// Repeat-guard bookkeeping for rescues run as their own script thread
/// (manual `run rescue_*` or the idle drain below): single level while
/// they run, guard set at their end position unless the operator stopped
/// them mid-flight.
pub(crate) fn finish_rescue(shared: &Shared, script: &str) {
    if !shared.stop_requested.load(Ordering::SeqCst) && !shared.inner.lock().unwrap().closing {
        let pos = shared.bot.lock().unwrap().position();
        *shared.rescue_done.lock().unwrap() = Some((script.to_string(), pos));
    }
    shared.rescue_gate.store(false, Ordering::SeqCst);
}

/// Observation thread: whole session life, idle and running. The live bot
/// is only borrowable when idle (the script thread holds it otherwise),
/// so running scripts are read from the heartbeat snapshot: enough for
/// geography and the carried cube, which is exactly the telemetry a
/// stuck script needs. Never the full `check()`: that walks and pumps,
/// which would recurse from inside a pump hook. E5: 1 s cadence — with
/// the resting serve loop, an idle worker wakes <= 2/s.
pub(crate) fn observe_events(shared: &Arc<Shared>) {
    loop {
        std::thread::sleep(Duration::from_millis(1000));
        if shared.inner.lock().unwrap().closing {
            return;
        }
        if shared.rescue_gate.load(Ordering::SeqCst) {
            continue;
        }
        if !shared.inner.lock().unwrap().connected {
            continue;
        }
        if let Ok(mut bot) = shared.bot.try_lock() {
            // Hook-safe greet only: adjacent hosts, no walking.
            let _ = lifescape_scripts::randomevent_full::greet_near(&mut bot, 300);
            if let Some(kind) = detect_live(&bot) {
                let pos = bot.position();
                let tick = bot.world.ticks;
                drop(bot);
                pose(shared, kind, pos, tick);
            } else {
                drop(bot);
                clear_rescue_state(shared);
            }
        } else if let Some(snapshot) = shared.heartbeat.lock().unwrap().clone() {
            let tick = snapshot.ticks;
            match detect_snapshot(&snapshot) {
                Some((kind, pos)) => pose(shared, kind, pos, tick),
                None => clear_rescue_state(shared),
            }
        }
    }
}

/// Idle rescue: no script thread exists, so the serve thread poses the
/// pending rescue as a real script run (same prelude as `run`, the rescue
/// thread owns the bot like any script). True when a run was spawned.
pub(crate) fn drain_idle(shared: &Arc<Shared>) -> bool {
    if shared.rescue_gate.load(Ordering::SeqCst) {
        return false;
    }
    let pending = shared.pending_rescue.lock().unwrap().take();
    let Some(pending) = pending else {
        return false;
    };
    // Defaut 4: bounded idle rescue. Over budget, fail typed and stay
    // idle: the brain replans the graph instead of looping the tower.
    let now = shared.inner.lock().unwrap().last_heartbeat_tick;
    if rescue_expired(pending.started, now) {
        fail_rescue_budget(shared, &pending, now);
        return false;
    }
    let launch = {
        let mut inner = shared.inner.lock().unwrap();
        if inner.script_alive || inner.held || !inner.connected || inner.closing {
            false
        } else if shared.rescue_done.lock().unwrap().as_ref().is_some_and(|done| done.0 == pending.rescue && done.1 == pending.pos) {
            false
        } else {
            inner.phase = "running".to_string();
            inner.script = Some(pending.rescue.clone());
            inner.script_args = Vec::new();
            inner.status = "running".to_string();
            inner.last_code = None;
            inner.last_error = None;
            inner.script_alive = true;
            true
        }
    };
    if !launch {
        *shared.pending_rescue.lock().unwrap() = Some(pending);
        return false;
    }
    crate::session::spawn_script_thread(shared, &pending.rescue, &[]);
    true
}

/// Idle market delivery: same prelude as the idle rescue (the delivery
/// thread owns the bot like any script). Rescue poses drain first at
/// the call site; when idle, order needs no gate.
pub(crate) fn drain_idle_market(shared: &Arc<Shared>) -> bool {
    if shared.rescue_gate.load(Ordering::SeqCst) {
        return false;
    }
    let pending = shared.pending_market.lock().unwrap().take();
    let Some(pending) = pending else {
        return false;
    };
    let launch = {
        let mut inner = shared.inner.lock().unwrap();
        if inner.script_alive || inner.held || !inner.connected || inner.closing {
            false
        } else {
            inner.phase = "running".to_string();
            inner.script = Some(pending.script.clone());
            inner.script_args = pending.args.clone();
            inner.status = "running".to_string();
            inner.last_code = None;
            inner.last_error = None;
            inner.script_alive = true;
            true
        }
    };
    if !launch {
        *shared.pending_market.lock().unwrap() = Some(pending);
        return false;
    }
    crate::session::spawn_script_thread(shared, &pending.script, &pending.args);
    true
}

/// Idle detection for the serve thread: never blocks (the script or a
/// rescue thread may hold the bot), feeds the same `pending_rescue`.
pub(crate) fn idle_scan(shared: &Shared) {
    let found = (|| {
        let bot = shared.bot.try_lock().ok()?;
        detect_live(&bot).map(|kind| (kind, bot.position(), bot.world.ticks))
    })();
    if let Some((kind, pos, tick)) = found {
        pose(shared, kind, pos, tick);
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_live, detect_snapshot, pose, rescue_dossier_path, rescue_expired, RESCUE_BUDGET_TICKS};
    use crate::session::{SessionInner, Shared};
    use lifescape_sdk::client::Snapshot;
    use lifescape_sdk::Bot;
    use lifescape_scripts::randomevent::{self, EventKind};
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    fn idle_bot_at(level: i32, x: i32, z: i32) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = level;
        bot.world.x = x;
        bot.world.z = z;
        bot
    }

    fn test_shared(bot: Bot) -> Shared {
        Shared {
            account: "t01".to_string(),
            log_path: std::path::PathBuf::from("/tmp/opencode-rescue-test.log"),
            state_path: std::path::PathBuf::from("/tmp/opencode-rescue-test.bot.json"),
            inner: Mutex::new(SessionInner {
                phase: "idle".to_string(),
                script: None,
                script_args: Vec::new(),
                paused: false,
                held: false,
                connected: true,
                status: "idle".to_string(),
                last_error: None,
                last_event: None,
                last_code: None,
                closing: false,
                script_alive: false,
                last_heartbeat_tick: 0,
                last_heartbeat_wall: 0.0,
                last_full_tick: 0,
                last_full_wall: 0.0,
                last_full_script: String::new(),
                last_full_status: String::new(),
                last_msg_logged: 0,
                last_zone: "?".to_string(),
                trail: Vec::new(),
            }),
            bot: Mutex::new(bot),
            heartbeat: Mutex::new(None),
            queue: Mutex::new(VecDeque::new()),
            stop_requested: AtomicBool::new(false),
            budget_stop: AtomicBool::new(false),
            task_start: Mutex::new(None),
            last_bank: Mutex::new(HashMap::new()),
            bank_baseline: Mutex::new(None),
            market_pause_ticks: Mutex::new(0),
            pending_market: Mutex::new(None),
            pending_rescue: Mutex::new(None),
            rescue_gate: AtomicBool::new(false),
            rescue_done: Mutex::new(None),
        }
    }

    #[test]
    fn live_geography_detects_maze_and_tower() {
        assert_eq!(detect_live(&idle_bot_at(0, 2900, 4560)), Some(EventKind::Maze));
        assert_eq!(detect_live(&idle_bot_at(1, 3278, 3500)), Some(EventKind::Tower));
        assert_eq!(detect_live(&idle_bot_at(0, 3220, 3220)), None);
    }

    /// A streamed event host poses the walking greet rescue: the
    /// adjacent-only `greet_near` cannot reach it, and an ignored dwarf
    /// escalates to stones.
    #[test]
    fn live_host_detection_poses_greet() {
        use lifescape_sdk::world::NpcInfo;
        let mut bot = idle_bot_at(0, 3220, 3220);
        lifescape_scripts::attach_live_tables(&mut bot);
        assert_eq!(detect_live(&bot), None, "no host in view");
        let dwarf = (0..9000usize)
            .find(|id| {
                lifescape_gamedata::npc_by_id(*id)
                    .and_then(|entry| entry.name)
                    .is_some_and(|name| name.eq_ignore_ascii_case("macro_dwarf"))
            })
            .expect("macro_dwarf in the npc dump") as u16;
        bot.world.npcs.insert(
            7,
            NpcInfo { npc_type: dwarf, x: 3222, z: 3220, level: 0, name: None, anim: None },
        );
        assert_eq!(detect_live(&bot), Some(EventKind::Host));
        assert_eq!(randomevent::rescue_for(EventKind::Host), "rescue_greet");
    }

    #[test]
    fn snapshot_detects_mime_and_cube() {
        let mut mime = Snapshot::default();
        mime.position = (0, 2000, 4760);
        assert_eq!(detect_snapshot(&mime).map(|(kind, _)| kind), Some(EventKind::Mime));

        let cube = lifescape_gamedata::objs_by_name("macro_cube").into_iter().next().expect("macro_cube in dumps");
        let mut pack = Snapshot::default();
        pack.position = (0, 3220, 3220);
        pack.backpack_com = Some(1);
        pack.inv.insert(1, [(0u8, (cube.obj_id as u16, 1u32))].into_iter().collect());
        assert_eq!(detect_snapshot(&pack).map(|(kind, _)| kind), Some(EventKind::Cube));

        let mut clear = Snapshot::default();
        clear.position = (0, 3220, 3220);
        assert_eq!(detect_snapshot(&clear), None);
    }

    #[test]
    fn pose_posts_once_per_spot() {
        let shared = test_shared(idle_bot_at(0, 3220, 3220));
        pose(&shared, EventKind::Maze, (0, 2900, 4560), 7);
        pose(&shared, EventKind::Maze, (0, 2900, 4560), 8);
        let pending = shared.pending_rescue.lock().unwrap();
        let pending = pending.as_ref().expect("posed once");
        assert_eq!(pending.rescue, "rescue_maze");
        assert_eq!(pending.pos, (0, 2900, 4560));
        let event = shared.inner.lock().unwrap().last_event.clone().expect("telemetry posed");
        assert_eq!(event.get("event").as_str(), Some("maze"));
        assert_eq!(event.get("resolved").as_bool(), Some(false));
    }

    #[test]
    fn gate_idles_without_pending() {
        let shared = test_shared(idle_bot_at(0, 3220, 3220));
        let mut bot = idle_bot_at(0, 3220, 3220);
        assert!(super::gate_tick(&shared, &mut bot));
        assert!(!shared.rescue_gate.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Pause, not re-run: a posed rescue runs inline on the same thread
    /// and the script loop resumes where it was (iteration counter keeps
    /// climbing on the same bot, pending consumed once).
    #[test]
    fn gate_runs_posed_rescue_inline_and_resumes() {
        let shared = test_shared(idle_bot_at(0, 3220, 3220));
        let mut bot = idle_bot_at(0, 3220, 3220);
        lifescape_scripts::attach_live_tables(&mut bot);
        // Cube rescue with an empty pack clears without touching the net:
        // offline-safe, still exercises pose -> inline run -> resume.
        pose(&shared, EventKind::Cube, (0, 3220, 3220), 7);
        let mut iterations = 0;
        for _ in 0..3 {
            assert!(super::gate_tick(&shared, &mut bot), "loop resumes after the inline rescue");
            iterations += 1;
        }
        assert_eq!(iterations, 3, "same thread continues, no re-run from the start");
        assert!(shared.pending_rescue.lock().unwrap().is_none(), "pending consumed once");
        let done = shared.rescue_done.lock().unwrap().clone().expect("rescue recorded");
        assert_eq!(done.0, "rescue_cube");
        assert_eq!(done.1, (0, 3220, 3220));
    }

    /// Every event maps to a runnable rescue: no fallback, no unknown key.
    #[test]
    fn every_event_maps_to_a_runnable_rescue() {
        for kind in [EventKind::Maze, EventKind::Mime, EventKind::Cube, EventKind::Tower, EventKind::Host] {
            let rescue = randomevent::rescue_for(kind);
            assert_ne!(rescue, "rescue_fallback", "{kind:?} must map, not fall back");
            assert!(lifescape_scripts::names().contains(&rescue), "mapped rescue {rescue} is runnable");
            let key = randomevent::rescue_key(kind);
            assert!(["maze", "mime", "cube", "down", "greet"].contains(&key), "short key {key} runs via run_rescue");
        }
    }

    /// Market pause, not re-run: a posed delivery runs inline on the same
    /// thread and the loop resumes where it was, pending consumed once.
    /// `rescue_cube` with an empty pack clears without touching the net:
    /// offline-safe, still exercises pose -> inline run -> resume.
    #[test]
    fn gate_runs_posed_market_inline_and_resumes() {
        let shared = test_shared(idle_bot_at(0, 3220, 3220));
        let mut bot = idle_bot_at(0, 3220, 3220);
        lifescape_scripts::attach_live_tables(&mut bot);
        super::pose_market(&shared, "rescue_cube", &[], 7);
        let mut iterations = 0;
        for _ in 0..3 {
            assert!(super::gate_tick(&shared, &mut bot), "loop resumes after the inline market run");
            iterations += 1;
        }
        assert_eq!(iterations, 3, "same thread continues, no re-run from the start");
        assert!(shared.pending_market.lock().unwrap().is_none(), "pending consumed once");
    }

    /// Off the safe point (modal open) the market pose waits: pending
    /// kept, loop alive. At the safe point it runs and clears.
    #[test]
    fn market_pose_defers_off_the_safe_point() {
        let shared = test_shared(idle_bot_at(0, 3220, 3220));
        let mut bot = idle_bot_at(0, 3220, 3220);
        lifescape_scripts::attach_live_tables(&mut bot);
        bot.world.modal = Some(lifescape_sdk::world::Modal { layer: "chat".to_string(), interface_id: 1 });
        super::pose_market(&shared, "rescue_cube", &[], 7);
        assert!(super::gate_tick(&shared, &mut bot), "loop alive while deferred");
        assert!(shared.pending_market.lock().unwrap().is_some(), "pose kept off the safe point");
        bot.world.modal = None;
        assert!(super::gate_tick(&shared, &mut bot), "loop resumes after the inline run");
        assert!(shared.pending_market.lock().unwrap().is_none(), "pose consumed at the safe point");
    }

    /// One level, rescue first: with both posed the rescue runs inline
    /// before the market pose, which waits its turn.
    #[test]
    fn rescue_runs_before_market_on_one_level() {
        let shared = test_shared(idle_bot_at(0, 3220, 3220));
        let mut bot = idle_bot_at(0, 3220, 3220);
        lifescape_scripts::attach_live_tables(&mut bot);
        pose(&shared, EventKind::Cube, (0, 3220, 3220), 7);
        super::pose_market(&shared, "rescue_cube", &[], 7);
        assert!(super::gate_tick(&shared, &mut bot), "rescue runs first, loop alive");
        assert!(shared.pending_rescue.lock().unwrap().is_none(), "rescue consumed first");
        assert!(shared.pending_market.lock().unwrap().is_some(), "market waits its turn");
        assert!(super::gate_tick(&shared, &mut bot), "market runs next, loop alive");
        assert!(shared.pending_market.lock().unwrap().is_none(), "market consumed second");
        assert!(!shared.rescue_gate.load(std::sync::atomic::Ordering::SeqCst), "single level released");
    }

    /// Defaut 4: the tower loop is bounded. 1800 ticks, not 90 minutes:
    /// at the budget the pose fails typed (dossier + RescueBudget) and
    /// the same spot never re-poses.
    #[test]
    fn rescue_budget_bounds_the_tower_loop() {
        assert_eq!(RESCUE_BUDGET_TICKS, 1800, "budget pinned");
        assert!(!rescue_expired(100, 100 + RESCUE_BUDGET_TICKS - 1), "one tick under keeps running");
        assert!(rescue_expired(100, 100 + RESCUE_BUDGET_TICKS), "at budget fails typed");
        assert!(!rescue_expired(1439, 5), "tick reset saturates, never fakes an expiry");
        let path = rescue_dossier_path("olivia_king", 9000);
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("olivia_king-rescue-9000.json"), "dossier rejouable, got {path:?}");
    }

    #[test]
    fn expired_pose_never_reposes_the_same_spot() {
        let shared = test_shared(idle_bot_at(1, 3278, 3500));
        pose(&shared, EventKind::Tower, (1, 3278, 3500), 100);
        assert!(shared.pending_rescue.lock().unwrap().is_some(), "tower posed");
        // Same spot at the budget: expire loudly instead of refreshing.
        pose(&shared, EventKind::Tower, (1, 3278, 3500), 100 + RESCUE_BUDGET_TICKS);
        assert!(shared.pending_rescue.lock().unwrap().is_none(), "expired pose consumed");
        let done = shared.rescue_done.lock().unwrap().clone().expect("repeat guard set");
        assert_eq!(done.0, "rescue_down", "tower rescue named, got {done:?}");
        // Still on the fail spot: no re-pose, the graph resumes instead.
        pose(&shared, EventKind::Tower, (1, 3278, 3500), 100 + RESCUE_BUDGET_TICKS + 1);
        assert!(shared.pending_rescue.lock().unwrap().is_none(), "same spot stays quiet after the budget");
        let error = shared.inner.lock().unwrap().last_error.clone().expect("typed failure");
        assert_eq!(error.get("type").as_str(), Some("RescueBudget"), "typed, got {error:?}");
    }
}
