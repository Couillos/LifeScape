//! Temporary, typed skill bans (user decision 2026-11-13). A goal that
//! ends stuck bans each skill it trained for `SKILL_BAN_TICKS`, never for
//! life (live: 12 of 25 bots had every skill banned by the old
//! `stuck:<skills>` entries and idled for good). The ban carries its cause
//! (root node, class, last error) so an operator can find and fix it.
//!
//! History shapes (`node: "goal"`):
//! - `stuck:<skill>@<ticks>:<class> (node <id>, <last_error>)` — banned for
//!   `<ticks>` ticks from the entry's `ended` stamp;
//! - `stuck:<a>,<b>` (legacy, no tick) — read back as already expired;
//! - `unban:<skill>` — the expiry was noted (logged once by the brain).
//!
//! Pure: elapsed ticks since a stamp come from the caller (`ticks_since`,
//! the brain maps wall seconds with the measured tick length).

use std::collections::BTreeMap;

use crate::lifecycle::{MAX_BLOCKS, MAX_FAILURES};
use crate::model::{HistoryEntry, Life, NodeStatus};

/// Length of a skill ban (D1 ticks: 1 h in prod at 600 ms, 6 min in dev).
pub const SKILL_BAN_TICKS: u64 = 6000;
/// History tag of a noted ban expiry.
pub const UNBAN_TAG: &str = "unban:";
/// Longest `last_error` kept in a ban entry (history stays readable).
const CAUSE_MAX_CHARS: usize = 200;

/// Why a goal ended stuck: the root of the latest abandonment.
#[derive(Debug, Clone, PartialEq)]
pub struct StuckCause {
    pub node: String,
    /// `3 failures`, `4 blocks` or `contract error` (Usage, no script,
    /// unworkable resolve: abandoned at once).
    pub class: String,
    pub last_error: String,
}

impl StuckCause {
    /// `<class> (node <id>, <last_error>)`: the log and history wording.
    pub fn describe(&self) -> String {
        format!("{} (node {}, {})", self.class, self.node, self.last_error)
    }
}

/// State of one skill read back from the history (latest entry wins).
#[derive(Debug, Clone, PartialEq)]
pub enum BanState {
    /// Banned for `ticks_left` more ticks.
    Active { ticks_left: u64, cause: String },
    /// Over, expiry not noted yet: the brain logs it and writes `unban:`.
    /// `legacy` = an old entry without a tick (counts as expired).
    Expired { cause: String, legacy: bool },
}

/// Root of the latest abandonment: the last `abandoned` history entry
/// (else the first abandoned node of the graph), followed through
/// cascade lines (`dependency nX abandoned`) to the node that failed.
pub fn stuck_cause(life: &Life) -> Option<StuckCause> {
    let abandoned = |id: &str| life.nodes.iter().find(|node| node.id == id && node.status == NodeStatus::Abandoned);
    let latest = life.history.iter().rev().filter(|entry| entry.outcome == "abandoned").find_map(|entry| abandoned(&entry.node));
    let mut node = latest.or_else(|| {
        let roots = life.nodes.iter().filter(|node| node.status == NodeStatus::Abandoned);
        roots.clone().find(|node| !node.last_error.starts_with("dependency ")).or_else(|| roots.clone().next())
    })?;
    for _ in 0..life.nodes.len() {
        let parent = node.last_error.strip_prefix("dependency ").and_then(|rest| rest.strip_suffix(" abandoned")).and_then(|id| abandoned(id));
        match parent {
            Some(parent) => node = parent,
            None => break,
        }
    }
    let class = if node.failures >= MAX_FAILURES {
        format!("{} failures", node.failures)
    } else if node.blocks > MAX_BLOCKS {
        format!("{} blocks", node.blocks)
    } else {
        "contract error".to_string()
    };
    let mut last_error: String = node.last_error.replace(['\n', '\r'], " ").chars().take(CAUSE_MAX_CHARS).collect();
    if last_error.is_empty() {
        last_error = "?".to_string();
    }
    Some(StuckCause { node: node.id.clone(), class, last_error })
}

/// History outcome of one ban (`skill` empty for a goal without skills:
/// the cause is kept, nothing is banned).
pub fn ban_entry(skill: &str, ticks: u64, cause: &StuckCause) -> String {
    format!("{}{skill}@{ticks}:{}", crate::goals::STUCK_TAG, cause.describe())
}

/// History outcome of a noted expiry.
pub fn unban_entry(skill: &str) -> String {
    format!("{UNBAN_TAG}{skill}")
}

/// Every skill whose latest ban entry is not lifted, sorted by name.
pub fn ban_states(history: &[HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<(String, BanState)> {
    let mut latest: BTreeMap<String, Option<BanState>> = BTreeMap::new();
    for entry in history.iter().filter(|entry| entry.node == "goal") {
        if let Some(skill) = entry.outcome.strip_prefix(UNBAN_TAG) {
            latest.insert(skill.to_string(), None);
            continue;
        }
        let Some(rest) = entry.outcome.strip_prefix(crate::goals::STUCK_TAG) else { continue };
        match rest.split_once('@') {
            Some((skill, tail)) => {
                let (raw_ticks, cause) = tail.split_once(':').unwrap_or((tail, ""));
                let ticks = raw_ticks.parse::<u64>().unwrap_or(0);
                if skill.is_empty() {
                    continue;
                }
                let elapsed = ticks_since(entry.ended);
                let state = if elapsed < ticks {
                    BanState::Active { ticks_left: ticks - elapsed, cause: cause.to_string() }
                } else {
                    BanState::Expired { cause: cause.to_string(), legacy: false }
                };
                latest.insert(skill.to_string(), Some(state));
            }
            None => {
                for skill in rest.split(',').filter(|skill| !skill.is_empty()) {
                    let cause = format!("legacy entry without tick ({})", entry.outcome);
                    latest.insert(skill.to_string(), Some(BanState::Expired { cause, legacy: true }));
                }
            }
        }
    }
    latest.into_iter().filter_map(|(skill, state)| state.map(|state| (skill, state))).collect()
}

/// Skills still banned now (the `avoid` list of goal drawing).
pub fn banned_skills(history: &[HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<String> {
    ban_states(history, ticks_since)
        .into_iter()
        .filter(|(_, state)| matches!(state, BanState::Active { .. }))
        .map(|(skill, _)| skill)
        .collect()
}

#[cfg(test)]
#[path = "bans_tests.rs"]
mod tests;
