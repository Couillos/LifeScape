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

    #[test]
    fn a_contended_block_names_its_site() {
        assert_eq!(contended_site(&["contended:3046,9734,0".to_string()]), Some((3046, 9734, 0)));
        assert_eq!(contended_site(&["coal".to_string()]), None);
        assert_eq!(contended_site(&["contended:x".to_string()]), None);
    }
}
