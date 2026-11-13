#![forbid(unsafe_code)]
//! Brain side of two user decisions (2026-11-13), pure rules in the
//! planner (`quests.rs`, `bans_scope.rs`):
//! - quests the bot finished: engine save varps (quest varps never reach
//!   the client) plus the life's done quest nodes, read before a launch;
//! - market rescue: before a stuck goal bans anything, each abandoned
//!   obtain on the failed path is checked for a market route (live book
//!   or reference price, the bot's free wallet) and the first one found
//!   is bought instead.

use lifescape_json::Value;
use lifescape_planner::{GraphState, Life, NodeKind};

use crate::brain::push_history;
use crate::jobs::Job;

/// F2P quests the account finished: the engine save (written at logout
/// and every 15 min) and the life's done quest nodes in between.
pub(crate) fn quests_done(life: &Life) -> Vec<String> {
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
    crate::logln!("brain {}: {id} quest {quest} already complete (engine save), done without launch", life.account);
    lifescape_planner::complete_node(life, position, 1);
    push_history(life, &id, "already_done");
    lifescape_planner::normalize(life);
    true
}

/// First pass of a stuck spell: buy the first item on the failed path the
/// market can sell (never twice the same node), before the goal closes
/// and its dependency subtree is banned. Returns true when a node was
/// rescued (the graph runs again).
pub(crate) fn market_rescue(job: &Job, life: &mut Life, live: &Value) -> bool {
    if !matches!(lifescape_planner::graph_state(life), GraphState::Stuck { .. }) || crate::brain_stuck::spell(&life.history) != crate::brain_stuck::Spell::Fresh {
        return false;
    }
    let Some(cause) = lifescape_planner::stuck_cause(life) else { return false };
    let wallet = crate::brain_plan::holdings_count(job, "coins");
    for position in lifescape_planner::rescue_candidates(life, &cause.node) {
        let node = life.nodes[position].clone();
        let NodeKind::ObtainItems { item, count } = &node.kind else { continue };
        let mut ctx = crate::brain_plan::ready_ctx(life, &node);
        crate::brain_market::fill_ctx(&mut ctx, &node, live);
        let held = crate::brain_plan::holdings_count(job, item);
        let committed = crate::brain_gear::committed_coins(&job.account, crate::brain_gear::obj_id(item));
        let coins = (wallet - committed).max(0);
        let need = (*count - held).max(1);
        if !lifescape_planner::market_route(need, coins, ctx.best_ask, ctx.price) {
            crate::logln!(
                "brain {}: no market route for {item} x{need} (ask {:?}, reference {}, free coins {coins})",
                life.account,
                ctx.best_ask,
                ctx.price
            );
            continue;
        }
        for change in lifescape_planner::retarget_market(life, position) {
            crate::logln!("brain {}: {change}", life.account);
        }
        crate::logln!("brain {}: {item} bought on the market instead of banning (root {}: {})", life.account, cause.node, cause.last_error);
        push_history(life, &node.id, "market_rescue");
        return true;
    }
    false
}
