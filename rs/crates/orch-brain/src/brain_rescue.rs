#![forbid(unsafe_code)]
//! Brain side of two user decisions (2026-11-13), pure rules in the
//! planner (`quests.rs`, `bans_scope.rs`):
//! - quests the bot finished: engine save varps (quest varps never reach
//!   the client) plus the life's done quest nodes, read before a launch;
//! - market rescue: before a stuck goal bans anything, each abandoned
//!   obtain on the failed path is checked for a market route (live book
//!   or reference price, the bot's free wallet) and the first one found
//!   is bought instead; one whose own buy went unfilled is produced
//!   instead (never the same order twice), once per node.

use lifescape_planner::{Life, NodeKind};

use crate::brain::push_history;

/// F2P quests the account finished: the engine save (written at logout
/// and every 15 min) and the life's done quest nodes in between.
pub fn quests_done(life: &Life) -> Vec<String> {
    let mut done = lifescape_planner::quests_done_in_graph(life);
    if let Some(save) = lifescape_sdk::save::load_account(&life.account, None) {
        let varp_of = |id: i64| u16::try_from(id).ok().and_then(|key| save.varps.get(&key)).map(|value| i64::from(*value)).unwrap_or(0);
        for quest in lifescape_planner::quests_done_from(&varp_of) {
            if !done.contains(&quest) {
                done.push(quest);
            }
        }
    }
    done
}

/// A quest node whose quest the bot already finished closes without a
/// launch (the save says so). Returns true when it closed.
pub(crate) fn close_done_quest(life: &mut Life, position: usize, done: &[String]) -> bool {
    let NodeKind::CompleteQuest { quest } = &life.nodes[position].kind else { return false };
    if !done.contains(quest) {
        return false;
    }
    let id = life.nodes[position].id.clone();
    crate::logln!("brain {}: {id} quest {quest} already complete (engine save or an undisproved done quest node), done without launch", life.account);
    lifescape_planner::complete_node(life, position, 1);
    push_history(life, &id, "already_done");
    lifescape_planner::normalize(life);
    true
}

// The market rescue (first pass of a stuck spell: buy the first item on the
// failed path the market can sell, never twice the same node, before the
// goal closes and its dependency subtree is banned) is the planner's
// (`upkeep_graph::market_rescue`); its market reads are `LiveUpkeepFx`'s.
