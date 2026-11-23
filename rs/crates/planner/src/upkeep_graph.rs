#![forbid(unsafe_code)]
//! Life upkeep decisions (lot 4), fourth family: the graph state. The
//! stuck spell (a run of passes where nothing is launchable: noted once,
//! goal closed once, the draw retried as bans expire, the cooldown revive,
//! the cap), the effective state (a reached goal over older abandoned
//! nodes is Finished, never a spell) and the market rescue (user decision
//! 2026-11-13: before a stuck goal bans anything, an item on the failed
//! path the market can sell is bought instead). Pure: the life, the live
//! levels and the clock go in, the changes and the lines come out; the
//! rescue reads the market through a `RescueReader` the orchestrator
//! implements, and prints through a sink (it reads between its lines).

use crate::launch_apply::push_history;
use crate::upkeep::Clock;
use crate::{goal_closed, normalize, GoalKind, GraphState, HistoryEntry, Life, Node, NodeKind, MAX_REVIVES, REVIVE_COOLDOWN_TICKS};

/// History outcome that ends a spell for good (revive cap reached).
pub const FINAL: &str = "final";

/// State of the stuck spell read back from the history: the last entry
/// that is not goal bookkeeping decides.
#[derive(Debug, Clone, PartialEq)]
pub enum Spell {
    /// No spell open: the graph just turned stuck.
    Fresh,
    /// A spell opened at this unix second, waiting for the revive.
    Waiting(i64),
    /// The revive cap was reached: the operator decides.
    Final,
}

pub fn spell(history: &[HistoryEntry]) -> Spell {
    match history.iter().rev().find(|entry| entry.node != "goal") {
        Some(entry) if entry.node == "graph" && entry.outcome == "stuck" => Spell::Waiting(entry.ended),
        Some(entry) if entry.node == "graph" && entry.outcome == FINAL => Spell::Final,
        _ => Spell::Fresh,
    }
}

/// True when the live levels already meet the goal (skill goals: every
/// skill at its target). Other goals answer from the graph, never here.
pub fn goal_met(goal: &GoalKind, level_of: &dyn Fn(&str) -> u8) -> bool {
    match goal {
        GoalKind::MaxSkills { skills, level } => !skills.is_empty() && skills.iter().all(|skill| level_of(skill) >= *level),
        _ => false,
    }
}

/// True when the current goal is reached: the live levels meet a skill
/// goal, or the goal's own root node is done.
pub fn goal_reached(life: &Life, level_of: &dyn Fn(&str) -> u8) -> bool {
    goal_met(&life.goal, level_of) || crate::goal_root_done(life)
}

/// The graph state the brain acts on: a Stuck graph whose current goal is
/// reached is Finished (closed done, next goal drawn), never a stuck spell
/// (live 2026-11-13: 540 lines/h over 20 bots).
pub fn effective_state(life: &Life, level_of: &dyn Fn(&str) -> u8) -> GraphState {
    match crate::graph_state(life) {
        GraphState::Stuck { .. } if goal_reached(life, level_of) => GraphState::Finished,
        state => state,
    }
}

/// Root causes named in the stuck line (the rest is counted).
const SUMMARY_ROOTS: usize = 3;
/// Longest error quoted per root in the stuck line.
const SUMMARY_ERROR_CHARS: usize = 160;

/// The stuck line's tail, bounded whatever the graph holds: the count, how
/// many fell by cascade, and the first root causes (live 2026-11-14).
pub fn stuck_summary(life: &Life, abandoned: &[String]) -> String {
    let nodes: Vec<&Node> = life.nodes.iter().filter(|node| abandoned.contains(&node.id)).collect();
    let cascade = nodes.iter().filter(|node| node.last_error.starts_with("dependency ")).count();
    let roots: Vec<String> = nodes
        .iter()
        .filter(|node| !node.last_error.starts_with("dependency "))
        .map(|node| {
            let error: String = node.last_error.replace(['\n', '\r'], " ").chars().take(SUMMARY_ERROR_CHARS).collect();
            format!("{}={}", node.id, if error.is_empty() { "?".to_string() } else { error })
        })
        .collect();
    let shown = roots.iter().take(SUMMARY_ROOTS).cloned().collect::<Vec<_>>().join(" | ");
    let more = roots.len().saturating_sub(SUMMARY_ROOTS);
    let more = if more > 0 { format!(" (+{more} more roots)") } else { String::new() };
    format!("{} nodes ({cascade} by cascade); roots: {}{more}", nodes.len(), if shown.is_empty() { "?" } else { shown.as_str() })
}

/// A spell just opened: its line, whether the goal is stuck (not reached),
/// and whether it was already closed (then only the draw is retried).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedSpell {
    pub line: String,
    pub stuck: bool,
    pub closed: bool,
}

/// First pass of a spell: noted once (`graph: stuck` in the history). A
/// goal the bot already meets is done, whatever older abandoned nodes
/// linger in the graph.
pub fn open_spell(life: &mut Life, level_of: &dyn Fn(&str) -> u8, abandoned: &[String], now: i64) -> OpenedSpell {
    let line = format!("brain {}: graph stuck, nothing left to run; abandoned: {}", life.account, stuck_summary(life, abandoned));
    push_history(life, "graph", "stuck", now);
    let stuck = !goal_reached(life, level_of);
    let closed = goal_closed(&life.history);
    OpenedSpell { line, stuck, closed }
}

/// After the spell's draw: the line to print; the life is normalized when
/// a goal was drawn.
pub fn spell_drawn(life: &mut Life, drawn: Option<&str>, stuck: bool) -> String {
    match drawn {
        Some(label) => {
            let line = format!("brain {}: goal {} -> next goal {label}", life.account, if stuck { "stuck" } else { "done" });
            normalize(life);
            line
        }
        None => format!(
            "brain {}: no goal left to draw (see the reason above); the draw retries as bans expire, abandoned nodes retry after {REVIVE_COOLDOWN_TICKS} ticks (at most {MAX_REVIVES} times)",
            life.account
        ),
    }
}

/// After the retry of a closed goal's draw (bans may have expired):
/// `(a goal was drawn, the life changed, the line to print)`.
pub fn retry_drawn(life: &mut Life, drawn: Option<&str>, changed: bool) -> (bool, bool, Option<String>) {
    match drawn {
        Some(label) => {
            let line = format!("brain {}: stuck spell over, next goal {label}", life.account);
            normalize(life);
            (true, true, Some(line))
        }
        None => (false, changed, None),
    }
}

/// The cooldown revive of a spell opened at `since`: `None` while it waits;
/// else the lines (the revive written into the life, or the cap reached).
pub fn revive(life: &mut Life, since: i64, clock: Clock) -> Option<Vec<String>> {
    let waited = clock.ticks_since(since);
    if waited < REVIVE_COOLDOWN_TICKS {
        return None;
    }
    let changes = crate::revive_stuck(life, waited);
    let mut lines = Vec::new();
    if changes.is_empty() {
        lines.push(format!("brain {}: revive cap reached ({MAX_REVIVES} per node), standing by (operator decides)", life.account));
        push_history(life, "graph", FINAL, clock.now);
    } else {
        for change in &changes {
            lines.push(format!("brain {}: revive {change}", life.account));
        }
        push_history(life, "graph", "revived", clock.now);
    }
    Some(lines)
}

/// A reached goal is closed once (live: `done:` was re-pushed every pass),
/// then the draw is retried every pass.
pub fn finished_closing(life: &Life) -> bool {
    !goal_closed(&life.history)
}

/// After the draw of a finished goal: the line to print, if any.
pub fn finished_drawn(life: &mut Life, drawn: Option<&str>, closing: bool) -> Option<String> {
    match drawn {
        Some(label) => {
            let line = format!("brain {}: goal done -> next goal {label}", life.account);
            normalize(life);
            Some(line)
        }
        None if closing => Some(format!("brain {}: no goal left to draw (see the reason above), retrying every pass", life.account)),
        None => None,
    }
}

/// What the orchestrator reads of one rescue candidate's market.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteFacts {
    /// Best resting ask, when the book has one.
    pub best_ask: Option<i64>,
    /// The market reference price.
    pub price: i64,
    /// Units already held (free of the bot's own listings).
    pub held: i64,
    /// Coins the bot's other resting buys commit.
    pub committed: i64,
}

/// The reads of the rescue: lazy, in the order the rule asks them.
pub trait RescueReader {
    /// The bot's coins.
    fn wallet(&self) -> i64;
    /// The market facts of an obtain node.
    fn facts(&self, life: &Life, node: &Node) -> RouteFacts;
}

/// First pass of a stuck spell: buy the first item on the failed path the
/// market can sell (never twice the same node), before the goal closes and
/// its dependency subtree is banned; one whose own buy went unfilled is
/// produced instead. True when a node was rescued (the graph runs again).
pub fn market_rescue(life: &mut Life, reader: &dyn RescueReader, now: i64, log: &mut dyn FnMut(String)) -> bool {
    if !matches!(crate::graph_state(life), GraphState::Stuck { .. }) || spell(&life.history) != Spell::Fresh {
        return false;
    }
    let Some(cause) = crate::stuck_cause(life) else { return false };
    let wallet = reader.wallet();
    for position in crate::rescue_candidates(life, &cause.node) {
        let node = life.nodes[position].clone();
        let NodeKind::ObtainItems { item, count } = &node.kind else { continue };
        // Its own buy went unfilled: never the same order again, its
        // production legs instead (live fiona_park 2026-11-14), or the ban
        // follows when it has none.
        if crate::market_failed(&node) {
            let changes = crate::retarget_produce(life, position);
            if changes.is_empty() {
                log(format!("brain {}: {item} unfilled on the market and never produced, no rescue", life.account));
                continue;
            }
            for change in changes {
                log(format!("brain {}: {change}", life.account));
            }
            log(format!("brain {}: {item} produced instead of bought again (root {}: {})", life.account, cause.node, cause.last_error));
            push_history(life, &node.id, crate::PRODUCE_RESCUE_TAG, now);
            return true;
        }
        let facts = reader.facts(life, &node);
        let coins = (wallet - facts.committed).max(0);
        let need = (*count - facts.held).max(1);
        if !crate::market_route(need, coins, facts.best_ask, facts.price) {
            log(format!("brain {}: no market route for {item} x{need} (ask {:?}, reference {}, free coins {coins})", life.account, facts.best_ask, facts.price));
            continue;
        }
        for change in crate::retarget_market(life, position) {
            log(format!("brain {}: {change}", life.account));
        }
        log(format!("brain {}: {item} bought on the market instead of banning (root {}: {})", life.account, cause.node, cause.last_error));
        push_history(life, &node.id, crate::MARKET_RESCUE_TAG, now);
        return true;
    }
    false
}
