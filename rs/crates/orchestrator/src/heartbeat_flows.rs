#![forbid(unsafe_code)]
//! Two-flow heartbeat (E4): a light frame every second keeps the minimap
//! and the session list alive; the full snapshot goes out rarely, in
//! diffs that matter.
//!
//! Before: every 10 ticks or 1 s the worker serialised the whole bot
//! state (npcs, locs, inv, bank, chat…) and the daemon rewrote the merged
//! file — ~1.7 files/s/bot in dev, ~1/s in prod, 500 writes/s at 300.
//! After: light frames carry position/ticks/status only (~150 bytes);
//! full frames flow every 100 ticks, every 30 s, or on script/status
//! change. `bot.json` on disk follows the same rule (E8 throttles the
//! daemon merge to the same cadence).

use std::sync::Arc;

use lifescape_json::{Value, object, str_value};

use crate::session::{Shared, now_secs, write_heartbeat};
use crate::session_heartbeat::heartbeat_value;

/// Light cadence: the spectator list never looks older than this.
pub const LIGHT_WALL_SECS: f64 = 1.0;
/// Full snapshot cadences: tick-driven, wall backstop, or state change.
pub const FULL_TICKS: u64 = 100;
pub const FULL_WALL_SECS: f64 = 30.0;

/// Compact entity positions for the light flow: `[nid, x, z]` per NPC,
/// `[name, x, z]` per player, same selection as the full frame (12 tiles,
/// 25 NPCs; 20 named players). Names ride the full frames only — the
/// spectator keeps the last known name per `nid` — so a second of
/// movement costs tens of bytes instead of a whole snapshot.
pub fn light_entities(bot: &lifescape_sdk::Bot) -> (Value, Value) {
    let mut npcs: Vec<(i64, Value)> = bot
        .world
        .npcs
        .iter()
        .map(|(slot, npc)| {
            let dist = (npc.x - bot.world.x).abs() + (npc.z - bot.world.z).abs();
            (
                dist as i64,
                Value::Arr(vec![
                    Value::Int(i64::from(*slot)),
                    Value::Int(i64::from(npc.x)),
                    Value::Int(i64::from(npc.z)),
                ]),
            )
        })
        .filter(|(dist, _)| *dist <= 12)
        .collect();
    npcs.sort_by_key(|(dist, _)| *dist);
    let npcs: Vec<Value> = npcs.into_iter().take(25).map(|(_, row)| row).collect();
    let players: Vec<Value> = bot
        .world
        .players
        .values()
        .filter(|player| player.name.is_some())
        .take(20)
        .map(|player| {
            Value::Arr(vec![
                str_value(player.name.as_deref().unwrap_or("?")),
                Value::Int(i64::from(player.x)),
                Value::Int(i64::from(player.z)),
            ])
        })
        .collect();
    (Value::Arr(npcs), Value::Arr(players))
}

/// Minimal live state: enough for minimap, list and spectate routing.
/// `npcs_at`/`players_at` carry the moving entities every second: without
/// them the spectator redrew the followed bot at 1 Hz while the NPCs
/// around it only moved on a full frame (100 ticks, or 30 s in prod).
pub fn light_value(
    account: &str,
    script: &str,
    status: &str,
    position: (i32, i32, i32),
    ticks: u64,
    zone: &str,
    intention: &str,
    updated: f64,
    npcs_at: Value,
    players_at: Value,
) -> Value {
    object(vec![
        ("type", str_value("snapshot_light")),
        ("account", str_value(account)),
        ("script", str_value(script)),
        ("status", str_value(status)),
        (
            "position",
            Value::Arr(vec![
                Value::Int(i64::from(position.0)),
                Value::Int(i64::from(position.1)),
                Value::Int(i64::from(position.2)),
            ]),
        ),
        ("ticks", Value::Int(ticks as i64)),
        ("zone", str_value(zone)),
        ("intention", str_value(intention)),
        ("updated", Value::Float(updated)),
        ("npcs_at", npcs_at),
        ("players_at", players_at),
    ])
}

/// Merge a light frame into the cached full state: only the light keys
/// move, everything else (inv, bank, npcs…) stays from the last full.
pub fn merge_light(cached: &mut Value, light: &Value) {
    let Value::Obj(map) = cached else { return };
    for key in ["script", "status", "position", "ticks", "zone", "intention", "updated", "npcs_at", "players_at"] {
        let value = light.get(key);
        if !value.is_null() {
            map.insert(key.to_string(), value.clone());
        }
    }
}

/// Full frame due: 100 ticks moved, 30 s elapsed, or script/status
/// changed since the last full.
pub fn full_due(
    last_full_tick: u64,
    tick: u64,
    last_full_wall: f64,
    now: f64,
    last_script: &str,
    script: &str,
    last_status: &str,
    status: &str,
) -> bool {
    if last_script != script || last_status != status {
        return true;
    }
    tick.saturating_sub(last_full_tick) >= FULL_TICKS || now - last_full_wall >= FULL_WALL_SECS
}

/// Script-thread heartbeat section (E4 two-flow): full snapshot on the
/// slow cadence or state change, light frame otherwise. Called with the
/// borrowed bot and the trail collected by the pump hook.
pub(crate) fn pump_heartbeat(shared: &Shared, bot: &lifescape_sdk::Bot, trail: &[(i32, i32, u64)]) {
    let tick = bot.world.ticks;
    let inner_view = shared.inner.lock().unwrap();
    let script = inner_view.script.clone().unwrap_or_default();
    let status = inner_view.status.clone();
    let zone = inner_view.last_zone.clone();
    let full = full_due(
        inner_view.last_full_tick,
        tick,
        inner_view.last_full_wall,
        now_secs(),
        &inner_view.last_full_script,
        &script,
        &inner_view.last_full_status,
        &status,
    );
    drop(inner_view);
    if full {
        let state = heartbeat_value(bot, &shared.account, &script, trail);
        *shared.heartbeat.lock().unwrap() = Some(bot.snapshot());
        let mut inner = shared.inner.lock().unwrap();
        inner.last_heartbeat_tick = tick;
        inner.last_heartbeat_wall = now_secs();
        inner.last_full_tick = tick;
        inner.last_full_wall = now_secs();
        inner.last_full_script = script;
        inner.last_full_status = status;
        drop(inner);
        crate::session_push::push_snapshot(&shared.account, &state);
    } else {
        // Light flow: position, status and entity positions, merged
        // daemon-side (the spectator moves the whole scene at 1 Hz).
        let (npcs_at, players_at) = light_entities(bot);
        let light = light_value(
            &shared.account,
            &script,
            &status,
            (bot.world.level, bot.world.x, bot.world.z),
            tick,
            &zone,
            &bot.intent,
            now_secs(),
            npcs_at,
            players_at,
        );
        let mut inner = shared.inner.lock().unwrap();
        inner.last_heartbeat_tick = tick;
        inner.last_heartbeat_wall = now_secs();
        drop(inner);
        crate::session_push::push_light(&light);
    }
}

/// Idle-loop heartbeat section (E4): periodic flow is light; full frames
/// ride explicit state changes (`write_heartbeat`) or the full cadence.
pub(crate) fn idle_heartbeat_flow(shared: &Arc<Shared>, tick: u64) {
    let inner = shared.inner.lock().unwrap();
    let script = inner.script.clone().unwrap_or_default();
    let status = inner.status.clone();
    let full = full_due(
        inner.last_full_tick,
        tick,
        inner.last_full_wall,
        now_secs(),
        &inner.last_full_script,
        &script,
        &inner.last_full_status,
        &status,
    );
    drop(inner);
    if full {
        write_heartbeat(shared);
    } else {
        write_light(shared);
    }
}

/// Light idle heartbeat: position and status only, no snapshot rebuild,
/// merged daemon-side.
pub(crate) fn write_light(shared: &Arc<Shared>) {
    let Ok(bot) = shared.bot.try_lock() else { return };
    let inner = shared.inner.lock().unwrap();
    let script = inner.script.clone().unwrap_or_default();
    let status = inner.status.clone();
    let zone = inner.last_zone.clone();
    let tick = bot.world.ticks;
    let position = (bot.world.level, bot.world.x, bot.world.z);
    let intent = bot.intent.clone();
    let (npcs_at, players_at) = light_entities(&bot);
    drop(bot);
    drop(inner);
    let now = now_secs();
    let light = light_value(&shared.account, &script, &status, position, tick, &zone, &intent, now, npcs_at, players_at);
    let mut inner = shared.inner.lock().unwrap();
    inner.last_heartbeat_tick = tick;
    inner.last_heartbeat_wall = now;
    drop(inner);
    crate::session_push::push_light(&light);
}

#[cfg(test)]
mod tests {
    use super::{full_due, light_value, merge_light};
    use lifescape_json::{object, str_value, Value};

    fn npc_row(nid: i64, x: i64, z: i64) -> Value {
        Value::Arr(vec![Value::Int(nid), Value::Int(x), Value::Int(z)])
    }

    /// Light frames are tiny and carry exactly the list keys.
    #[test]
    fn light_frame_stays_tiny() {
        let light = light_value(
            "t01", "woodcut", "running", (0, 3228, 3214), 700, "Lumbridge", "chop", 1.0,
            Value::Arr(Vec::new()), Value::Arr(Vec::new()),
        );
        assert_eq!(light.get("type").as_str(), Some("snapshot_light"));
        assert_eq!(light.get("ticks").as_i64(), Some(700));
        let wire = lifescape_json::stringify(&light);
        assert!(wire.len() < 400, "list frame must stay small, got {}", wire.len());
    }

    /// Live 2026-11-12 (user): in the spectator the followed bot moved
    /// every second while the NPCs and players around it only moved on a
    /// full frame — 100 ticks, i.e. 30 s in prod. Their positions ride the
    /// light flow now, and a loaded scene still fits in one small frame.
    #[test]
    fn light_frame_carries_entity_positions() {
        let npcs = Value::Arr((0..25).map(|index| npc_row(1200 + index, 3228 + index, 3214)).collect());
        let players = Value::Arr(
            (0..20)
                .map(|index| Value::Arr(vec![str_value("olivia_king"), Value::Int(3230 + index), Value::Int(3216)]))
                .collect(),
        );
        let light = light_value(
            "t01", "woodcut", "running", (0, 3228, 3214), 700, "Lumbridge", "chop", 1.0, npcs, players,
        );
        let rows = light.get("npcs_at").as_arr().cloned().unwrap_or_default();
        assert_eq!(rows.len(), 25, "every streamed npc rides the light frame");
        let first = rows[0].as_arr().cloned().unwrap_or_default();
        assert_eq!(first[0].as_i64(), Some(1200), "nid first, then x and z");
        let wire = lifescape_json::stringify(&light);
        assert!(wire.len() < 1500, "a full scene stays one small frame, got {}", wire.len());

        // The daemon merges the fresh positions into the cached full state.
        let mut cached = object(vec![("npcs", Value::Arr(Vec::new())), ("inv", Value::Arr(Vec::new()))]);
        merge_light(&mut cached, &light);
        assert_eq!(cached.get("npcs_at").as_arr().map(Vec::len), Some(25), "npc positions merged");
        assert_eq!(cached.get("players_at").as_arr().map(Vec::len), Some(20), "player positions merged");
        assert_eq!(cached.get("inv").as_arr().map(Vec::len), Some(0), "full-only keys survive");
    }

    /// Merge moves only the light keys; the last full keeps the rest.
    #[test]
    fn merge_keeps_the_last_full() {
        let mut cached = object(vec![
            ("inv", Value::Arr(vec![str_value("logs")])),
            ("ticks", Value::Int(600)),
            ("zone", str_value("old")),
        ]);
        let light = light_value(
            "t01", "woodcut", "running", (0, 3228, 3214), 700, "Lumbridge", "chop", 1.0,
            Value::Arr(Vec::new()), Value::Arr(Vec::new()),
        );
        merge_light(&mut cached, &light);
        assert_eq!(cached.get("ticks").as_i64(), Some(700));
        assert_eq!(cached.get("zone").as_str(), Some("Lumbridge"));
        assert_eq!(cached.get("inv").as_arr().map(Vec::len), Some(1), "full-only keys survive");
    }

    /// Full frames fire on change immediately, otherwise on the slow
    /// cadence — never on every tick.
    #[test]
    fn full_fires_on_change_or_slow_cadence() {
        assert!(full_due(0, 5, 0.0, 1.0, "a", "b", "running", "running"), "script change");
        assert!(full_due(0, 5, 0.0, 1.0, "a", "a", "idle", "running"), "status change");
        assert!(full_due(0, 100, 0.0, 1.0, "a", "a", "running", "running"), "100 ticks");
        assert!(full_due(0, 5, 0.0, 31.0, "a", "a", "running", "running"), "30 s wall");
        assert!(!full_due(0, 5, 0.0, 1.0, "a", "a", "running", "running"), "quiet stays light");
    }
}
