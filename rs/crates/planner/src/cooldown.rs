//! Per-bot method and site cooldowns (plan P1, 2026-11-17).
//!
//! A skill ban cools a list of skills, empty or unrelated for a money or
//! side goal (live: `fight:melee/men` failing "pack too full to hunt"
//! banned crafting and was resolved again at once, 3 031 relaunches in
//! 3 h). An abandoned node now cools what actually failed, for THIS bot:
//! the resolved method at its site (`method@x,z,level`), or the method
//! itself when it has no site or failed at a second site within the
//! cooldown. A contended site (D4, user 2026-11-17) cools the same way
//! without any failure counted. `ReadyCtx::cooled` carries the active
//! keys: a cooled method is unworkable like a quarantined one, a cooled
//! site is skipped by the site pick, for every goal kind.
//!
//! History shape (`node: "cool"`): `cool:<key>|<ticks>|<cause>`, active for
//! `<ticks>` ticks from the entry's `ended` stamp. Pure: elapsed ticks
//! come from the caller (`ticks_since`), like `bans`.

use crate::model::HistoryEntry;

/// History node of a cooldown entry.
pub const COOL_NODE: &str = "cool";
/// Outcome prefix of a cooldown entry.
pub const COOL_TAG: &str = "cool:";
/// Length of a cooldown (same as a skill ban: 1 h in prod, 6 min in dev).
pub const COOL_TICKS: u64 = crate::bans::SKILL_BAN_TICKS;
/// Longest cause kept in an entry.
const CAUSE_MAX_CHARS: usize = 160;

/// Key of a method at a site (`method@x,z,level`), or of the method alone.
pub fn cool_key(method: &str, site: Option<(i64, i64, i64)>) -> String {
    match site {
        Some((x, z, level)) => format!("{method}@{x},{z},{level}"),
        None => method.to_string(),
    }
}

/// Split a key into its method and its site (when it names one).
pub fn split_key(key: &str) -> (&str, Option<(i64, i64, i64)>) {
    let Some((method, site)) = key.rsplit_once('@') else { return (key, None) };
    let parts: Vec<i64> = site.split(',').filter_map(|part| part.trim().parse::<i64>().ok()).collect();
    match parts.as_slice() {
        [x, z, level] => (method, Some((*x, *z, *level))),
        _ => (key, None),
    }
}

/// History outcome of one cooldown.
pub fn cool_entry(key: &str, ticks: u64, cause: &str) -> String {
    let cause: String = cause.replace(['\n', '\r', '|'], " ").chars().take(CAUSE_MAX_CHARS).collect();
    format!("{COOL_TAG}{key}|{ticks}|{cause}")
}

/// Keys cooled now (latest entry per key wins), sorted and unique.
pub fn cooled_keys(history: &[HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in history.iter().filter(|entry| entry.node == COOL_NODE) {
        let Some(rest) = entry.outcome.strip_prefix(COOL_TAG) else { continue };
        let mut parts = rest.splitn(3, '|');
        let (Some(key), Some(raw_ticks)) = (parts.next(), parts.next()) else { continue };
        let ticks = raw_ticks.parse::<u64>().unwrap_or(0);
        if key.is_empty() || ticks_since(entry.ended) >= ticks {
            continue;
        }
        if !out.iter().any(|known| known == key) {
            out.push(key.to_string());
        }
    }
    out.sort();
    out
}

/// True when `method` itself is cooled (every site).
pub fn method_cooled(cooled: &[String], method: &str) -> bool {
    cooled.iter().any(|key| key == method)
}

/// Sites of `method` cooled for this bot.
pub fn cooled_sites(cooled: &[String], method: &str) -> Vec<(i64, i64, i64)> {
    cooled
        .iter()
        .filter_map(|key| match split_key(key) {
            (cooled_method, Some(site)) if cooled_method == method => Some(site),
            _ => None,
        })
        .collect()
}

/// True when two sites are one working area: same level, within
/// `combat::CROWD_RADIUS` (the radius the crowd and the capacity count
/// in). Tour 2 P2-3a: the cooldown keyed the ROCK, so cooling (3242,3160)
/// sent the bot to (3243,3166), the same swamp field.
pub fn same_area(left: (i64, i64, i64), right: (i64, i64, i64)) -> bool {
    left.2 == right.2 && (left.0 - right.0).abs().max((left.1 - right.1).abs()) <= crate::combat::CROWD_RADIUS
}

/// Keys an abandonment of `method` at `site` cools: the method at its
/// site; the method itself when it has no site, or when another site of
/// the same method is already cooled (the failure follows the method,
/// not the place).
pub fn keys_for_abandon(cooled: &[String], method: &str, site: Option<(i64, i64, i64)>) -> Vec<String> {
    if method.is_empty() {
        return Vec::new();
    }
    let Some(site) = site else { return vec![method.to_string()] };
    let mut keys = vec![cool_key(method, Some(site))];
    if cooled_sites(cooled, method).iter().any(|other| !same_area(*other, site)) {
        keys.push(method.to_string());
    }
    keys
}

/// History outcome of a node waiting out its cooled methods (tour 3,
/// point 1): `cool_wait:<method>`.
pub const WAIT_TAG: &str = "cool_wait:";

/// The method a resolve block names when every site of every candidate
/// is full (`full:<method>`, D6) or cooled (`cooled:<method>`): a block
/// the node waits out, never an abandonment.
pub fn resolve_wait(reason: &str) -> Option<&str> {
    reason.strip_prefix("full:").or_else(|| reason.strip_prefix("cooled:")).filter(|method| !method.is_empty())
}

/// What a resolve block on a method does (live 2026-11-17: 14:19
/// alice_knight `blocked at resolve (full:gather:mining/mithril_ore) ->
/// abandoned`, 13:00 owen_wood `cooled:farm:adamantite_ore`): the method
/// not cooled yet cools for this bot and the node resolves again at once
/// (its next method, D6); already cooled (every candidate is), the node
/// stays open and waits until the method's cooldown lapses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveWait {
    /// Cool the method itself, the next resolve picks another method.
    CoolAndRetry,
    /// Nothing left to try: the node waits on the method's cooldown.
    Wait,
}

/// `ResolveWait` for a block on `method` with `cooled` keys active.
pub fn resolve_wait_step(cooled: &[String], method: &str) -> ResolveWait {
    if method_cooled(cooled, method) {
        ResolveWait::Wait
    } else {
        ResolveWait::CoolAndRetry
    }
}

/// Nodes that wait on a cooled method: their latest history entry is a
/// `cool_wait:<method>` and `<method>` is still cooled. A lapsed cooldown
/// (or any later entry of the node) releases the node.
pub fn waiting_nodes(history: &[HistoryEntry], cooled: &[String]) -> Vec<String> {
    let mut latest: Vec<(&str, &str)> = Vec::new();
    for entry in history.iter().filter(|entry| entry.node != COOL_NODE) {
        match latest.iter_mut().find(|(node, _)| *node == entry.node) {
            Some(slot) => slot.1 = &entry.outcome,
            None => latest.push((&entry.node, &entry.outcome)),
        }
    }
    latest
        .into_iter()
        .filter_map(|(node, outcome)| outcome.strip_prefix(WAIT_TAG).filter(|method| method_cooled(cooled, method)).map(|_| node.to_string()))
        .collect()
}

/// Same error twice, the digits aside (positions, counts, ticks): the
/// deterministic failure test of `lifecycle::record_failure_timed`.
pub fn same_error(left: &str, right: &str) -> bool {
    let shape = |text: &str| -> String { text.chars().filter(|glyph| !glyph.is_ascii_digit()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ") };
    !left.is_empty() && shape(left) == shape(right)
}

#[cfg(test)]
#[path = "cooldown_tests.rs"]
mod tests;
