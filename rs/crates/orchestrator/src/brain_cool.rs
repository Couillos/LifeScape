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

/// Tag a script blocks with on a contended site (`kit::site::CONTENDED_TAG`).
pub(crate) const CONTENDED_TAG: &str = "contended:";

/// The contended site a block names (`contended:<x>,<z>,<level>`).
pub(crate) fn contended_site(needs: &[String]) -> Option<(i64, i64, i64)> {
    needs.iter().find_map(|need| {
        let parts: Vec<i64> = need.strip_prefix(CONTENDED_TAG)?.split(',').filter_map(|part| part.trim().parse::<i64>().ok()).collect();
        match parts.as_slice() {
            [x, z, level] => Some((*x, *z, *level)),
            _ => None,
        }
    })
}

/// Cool one contended site of `method` (D4): no failure counted, no
/// ban, the next resolve picks another site at once.
pub(crate) fn on_contended(life: &mut Life, method: &str, site: (i64, i64, i64)) {
    let key = lifescape_planner::cooldown::cool_key(method, Some(site));
    crate::logln!("brain {}: {key} contended, site cooled {COOL_TICKS} ticks for this bot (no ban, no failure)", life.account);
    push_history(life, COOL_NODE, &cool_entry(&key, COOL_TICKS, "contended: other players work every node"));
}

#[cfg(test)]
mod tests {
    use super::contended_site;

    #[test]
    fn a_contended_block_names_its_site() {
        assert_eq!(contended_site(&["contended:3046,9734,0".to_string()]), Some((3046, 9734, 0)));
        assert_eq!(contended_site(&["coal".to_string()]), None);
        assert_eq!(contended_site(&["contended:x".to_string()]), None);
    }
}
