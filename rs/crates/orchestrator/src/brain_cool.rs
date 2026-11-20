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

/// Longest contended cooldown, in plain cooldowns (16 × 6000 ticks).
pub(crate) const CONTENDED_MAX_FACTOR: u64 = 16;
/// Ticks an earlier contended cool of a site still lengthens the next.
pub(crate) const CONTENDED_MEMORY_TICKS: u64 = COOL_TICKS * CONTENDED_MAX_FACTOR * 2;
/// Word every contended cool's cause carries (node and filler alike).
const CONTENDED_CAUSE: &str = "contended";

/// Pure: ticks the next contended cool of `key` lasts. Each contended
/// cool of the same key younger than `CONTENDED_MEMORY_TICKS` doubles
/// the plain `COOL_TICKS`, up to `CONTENDED_MAX_FACTOR` (live
/// 2026-11-20: 5 900 of 8 183 contended blocks were the same bot back on
/// the same site the moment its cooldown ran out). A failure cool or
/// another site never counts.
pub(crate) fn contended_ticks(history: &[lifescape_planner::HistoryEntry], key: &str, ticks_since: &dyn Fn(i64) -> u64) -> u64 {
    let prefix = format!("{}{key}|", lifescape_planner::cooldown::COOL_TAG);
    let streak = history
        .iter()
        .filter(|entry| entry.node == COOL_NODE && ticks_since(entry.ended) < CONTENDED_MEMORY_TICKS)
        .filter_map(|entry| entry.outcome.strip_prefix(&prefix))
        .filter(|rest| rest.split_once('|').is_some_and(|(_, cause)| cause.contains(CONTENDED_CAUSE)))
        .count();
    let factor = 1u64.checked_shl(u32::try_from(streak).unwrap_or(u32::MAX)).unwrap_or(u64::MAX).min(CONTENDED_MAX_FACTOR);
    COOL_TICKS * factor
}

/// Cool one contended site of `method` (D4): no failure counted, no
/// ban, the next resolve picks another site at once. A site contended
/// again cools longer (`contended_ticks`).
pub(crate) fn on_contended(life: &mut Life, method: &str, site: (i64, i64, i64)) {
    let key = lifescape_planner::cooldown::cool_key(method, Some(site));
    let now = now_unix();
    let ticks = contended_ticks(&life.history, &key, &|stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp));
    crate::logln!("brain {}: {key} contended, site cooled {ticks} ticks for this bot (no ban, no failure)", life.account);
    push_history(life, COOL_NODE, &cool_entry(&key, ticks, "contended: other players work every node"));
}

/// Nodes waiting out a cooled method (tour 3 point 1): skipped by the
/// launch pick like a parked buy, a filler works meanwhile.
pub(crate) fn waiting(life: &Life) -> Vec<String> {
    lifescape_planner::cooldown::waiting_nodes(&life.history, &active(life))
}

/// A resolve block every site of every candidate refused (`full:` /
/// `cooled:<method>`): never an abandonment. The method not cooled yet
/// cools for this bot and the node resolves again at once (its next
/// method, D6); every candidate cooled, the node stays open and waits
/// out the cooldown. False when `reason` is no such block.
pub(crate) fn on_resolve_wait(life: &mut Life, index: usize, reason: &str) -> bool {
    use lifescape_planner::cooldown::{resolve_wait, resolve_wait_step, ResolveWait, WAIT_TAG};
    let Some(method) = resolve_wait(reason).map(str::to_string) else { return false };
    let id = life.nodes[index].id.clone();
    match resolve_wait_step(&active(life), &method) {
        ResolveWait::CoolAndRetry => {
            crate::logln!("brain {}: {method} cooled {COOL_TICKS} ticks for this bot: {reason} ({id} stays open, next method)", life.account);
            push_history(life, COOL_NODE, &cool_entry(&method, COOL_TICKS, reason));
        }
        ResolveWait::Wait => {
            crate::logln!("brain {}: {id} waits: {reason} (every method cooled, node stays open)", life.account);
            push_history(life, &id, &format!("{WAIT_TAG}{method}"));
        }
    }
    lifescape_planner::wait_open(life, index, reason);
    true
}

#[cfg(test)]
mod tests {
    use super::{contended_site, on_resolve_wait, waiting};
    use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

    fn life_with_blocked(reason: &str) -> Life {
        let mut node = Node::new("n1", NodeKind::ReachLevel { skill: "mining".to_string(), level: 70 }, Vec::new(), 70);
        node.status = NodeStatus::Blocked { reason: reason.to_string() };
        Life {
            schema: 1,
            account: "t_cool".to_string(),
            created_at: 0,
            seed: 1,
            profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
            goal: GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 70 },
            nodes: vec![node],
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        }
    }

    /// Tour 3 point 1 (live 2026-11-17 14:19 alice_knight `blocked at
    /// resolve (full:gather:mining/mithril_ore) -> abandoned`): the full
    /// method cools, the node stays open and ready (next method on the
    /// next resolve); full again with the method cooled, it waits open.
    #[test]
    fn a_full_resolve_never_abandons_the_node() {
        let reason = "full:gather:mining/mithril_ore";
        let mut life = life_with_blocked(reason);
        assert!(on_resolve_wait(&mut life, 0, reason));
        assert_eq!(life.nodes[0].status, NodeStatus::Ready);
        assert_eq!(life.nodes[0].failures, 0);
        assert!(super::active(&life).iter().any(|key| key == "gather:mining/mithril_ore"), "the method cooled");
        assert!(waiting(&life).is_empty(), "the next resolve tries the next method");
        life.nodes[0].status = NodeStatus::Blocked { reason: "cooled:gather:mining/mithril_ore".to_string() };
        assert!(on_resolve_wait(&mut life, 0, "cooled:gather:mining/mithril_ore"));
        assert_eq!(life.nodes[0].status, NodeStatus::Ready);
        assert_eq!(waiting(&life), vec!["n1".to_string()], "every method cooled: the node waits, open");
        assert!(!on_resolve_wait(&mut life, 0, "missing:coins:60"), "any other block is not a wait");
    }

    /// Live 2026-11-20: 8 183 contended blocks in 24 h, 5 900 of them the
    /// same bot back on the same site (liam_west on the gem trader 62
    /// times, felix_rook on the Wilderness big bones 58 times): the site
    /// came back the moment its 6000 ticks ran out (median gap 894 s at
    /// 60 ms). Each contended cool of the same key still remembered
    /// doubles the next one, up to `CONTENDED_MAX_FACTOR`; another site
    /// or a failure cool never counts.
    #[test]
    fn a_site_contended_again_cools_longer() {
        use lifescape_planner::cooldown::{cool_entry, COOL_NODE, COOL_TICKS};
        use lifescape_planner::HistoryEntry;
        let key = "ground:big_bones@3214,3733,0";
        let entry = |outcome: String, ended: i64| HistoryEntry { node: COOL_NODE.to_string(), started: ended, ended, outcome };
        let ticks_since = |stamp: i64| (1000 - stamp) as u64 * 10;
        let mut history = Vec::new();
        assert_eq!(super::contended_ticks(&history, key, &ticks_since), COOL_TICKS, "first contention: the plain cooldown");
        history.push(entry(cool_entry(key, COOL_TICKS, "blocked, missing: contended:3214,3733,0"), 900));
        assert_eq!(super::contended_ticks(&history, key, &ticks_since), 2 * COOL_TICKS);
        history.push(entry(cool_entry(key, 2 * COOL_TICKS, "contended: other players work every node"), 950));
        assert_eq!(super::contended_ticks(&history, key, &ticks_since), 4 * COOL_TICKS);
        history.push(entry(cool_entry("ground:big_bones@3000,3700,0", COOL_TICKS, "contended: other players work every node"), 960));
        history.push(entry(cool_entry(key, COOL_TICKS, "failed: no big_bones picked"), 970));
        assert_eq!(super::contended_ticks(&history, key, &ticks_since), 4 * COOL_TICKS, "another site, a failure: not counted");
        for stamp in 971..990 {
            history.push(entry(cool_entry(key, COOL_TICKS, "contended: other players work every node"), stamp));
        }
        assert_eq!(super::contended_ticks(&history, key, &ticks_since), super::CONTENDED_MAX_FACTOR * COOL_TICKS, "capped");
        let forgotten = |stamp: i64| if stamp < 1000 { super::CONTENDED_MEMORY_TICKS } else { 0 };
        assert_eq!(super::contended_ticks(&history, key, &forgotten), COOL_TICKS, "an old streak is forgotten");
        let mut life = life_with_blocked("missing:coal");
        super::on_contended(&mut life, "gather:mining/coal", (3298, 3317, 0));
        super::on_contended(&mut life, "gather:mining/coal", (3298, 3317, 0));
        let last = life.history.last().map(|entry| entry.outcome.clone()).unwrap_or_default();
        assert!(last.starts_with(&format!("cool:gather:mining/coal@3298,3317,0|{}|", 2 * COOL_TICKS)), "{last}");
    }

    #[test]
    fn a_contended_block_names_its_site() {
        assert_eq!(contended_site(&["contended:3046,9734,0".to_string()]), Some((3046, 9734, 0)));
        assert_eq!(contended_site(&["coal".to_string()]), None);
        assert_eq!(contended_site(&["contended:x".to_string()]), None);
    }
}
