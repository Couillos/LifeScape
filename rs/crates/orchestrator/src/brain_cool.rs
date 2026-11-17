//! Per-bot method and site cooldowns in the brain (plan P1, D4): stamps
//! the planner's `cooldown` entries into the life history and reads the
//! active keys back into `ReadyCtx::cooled`. Pure decisions in
//! `lifescape_planner::cooldown`; this file only stamps and logs.

#![forbid(unsafe_code)]

use lifescape_planner::cooldown::{cool_entry, cooled_keys, keys_for_abandon, COOL_NODE, COOL_TICKS};
use lifescape_planner::Life;

use crate::brain::push_history;

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Keys cooled for this life now.
pub(crate) fn active(life: &Life) -> Vec<String> {
    let now = now_unix();
    cooled_keys(&life.history, &|stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp))
}

/// Cool what an abandoned node ran (`method` at `site`): logged once per
/// key, written to the history. Returns the keys written.
pub(crate) fn on_abandon(life: &mut Life, method: &str, site: Option<(i64, i64, i64)>, cause: &str) -> Vec<String> {
    // Purchases and markers are never cooled: their failure is the
    // market's, judged by the rescue and fill paths.
    if method.is_empty() || method.starts_with("market:") || method.starts_with("shop:") || method.starts_with("dialogue:") {
        return Vec::new();
    }
    let keys = keys_for_abandon(&active(life), method, site);
    for key in &keys {
        crate::logln!("brain {}: {key} cooled {COOL_TICKS} ticks for this bot: {cause}", life.account);
        push_history(life, COOL_NODE, &cool_entry(key, COOL_TICKS, cause));
    }
    keys
}
