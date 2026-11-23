#![forbid(unsafe_code)]
//! Worker side of the market locks (user rule 2026-11-13): the daemon
//! pushes what the account's open orders and deals promise; the session
//! writes it into the bot's shared locks even while a script holds the
//! bot (one worker process = one account, so one handle per process).

use std::collections::HashMap;
use std::sync::OnceLock;

use lifescape_json::{object, Value};
use lifescape_sdk::client::locks::SharedLocks;
use lifescape_sdk::Bot;

static LOCKS: OnceLock<SharedLocks> = OnceLock::new();

/// Called once when the session builds its bot.
pub(crate) fn install(locks: SharedLocks) {
    let _ = LOCKS.set(locks);
}

/// `set_locks {coins, items: [[id, qty], ...]}`: replaces the pushed
/// locks; the trade flag of a running handover is kept.
pub(crate) fn cmd_set_locks(request: &Value) -> Value {
    let Some(shared) = LOCKS.get() else {
        return object(vec![("ok", Value::Bool(false)), ("error", lifescape_json::str_value("no bot"))]);
    };
    let coins = request.get("coins").as_i64().unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32;
    let mut items: HashMap<u16, u32> = HashMap::new();
    for row in request.get("items").as_arr().cloned().unwrap_or_default() {
        let pair = row.as_arr().cloned().unwrap_or_default();
        let (Some(id), Some(qty)) = (pair.first().and_then(Value::as_i64), pair.get(1).and_then(Value::as_i64)) else { continue };
        if let (Ok(id), true) = (u16::try_from(id), qty > 0) {
            items.insert(id, qty.min(i64::from(u32::MAX)) as u32);
        }
    }
    Bot::replace_locks(shared, items, coins);
    object(vec![("ok", Value::Bool(true))])
}
