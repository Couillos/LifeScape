#![forbid(unsafe_code)]
//! Life upkeep decisions (lot 4), third family: the ambition. The one-time
//! D3 mark (user 2026-11-19, then A2 of 2026-11-21: never a retirement),
//! the catalogue sync (a revised ladder drops the quotas it no longer
//! offers, the supply rule of 2026-11-20), the row of the one-time A4
//! rebalance a bot applies (2026-11-21, the only exception to A2) and the
//! seats of a bid goal after a restart. Pure: the life and the facts it is
//! given in, the changes and the lines out. The fleet-wide parts (assigning
//! every bot, the coverage log, the plan and its marker, the draws) stay in
//! the orchestrator.

use crate::ambition::BotNow;
use crate::{goal_closed, normalize, GoalKind, Life};

/// Every life holds an ambition (2026-11-13): the first pass that meets
/// one without assigns the whole fleet (the orchestrator).
pub fn lacks_ambition(life: &Life) -> bool {
    life.ambition.is_none()
}

/// One-time D3 mark (user 2026-11-19, then A2 of 2026-11-21): an ambition a
/// need-led bot holds without the `need` mark and drawn by the need before
/// the mark is only marked, never retired. Returns the line to print when
/// the life changed.
pub fn review_mark(life: &mut Life) -> Option<String> {
    let account = life.account.clone();
    let ambition = life.ambition.as_mut()?;
    if crate::ambition_review::adopt_need_mark(ambition) {
        return Some(format!("brain {account}: ambition {} ({}) drawn by need before the mark, marked need (D3)", ambition.kind, ambition.why));
    }
    None
}

/// The first half of a catalogue sync: the milestones the catalogue no
/// longer offers are dropped, the line printed before the fleet's piles are
/// read.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogDrop {
    pub dropped: Vec<String>,
    pub line: Option<String>,
}

/// Open ladder production milestones the catalogue no longer offers leave
/// the ambition. `None`: the life holds no ambition.
pub fn drop_stale(life: &mut Life) -> Option<CatalogDrop> {
    let account = life.account.clone();
    let ambition = life.ambition.as_mut()?;
    let dropped = crate::drop_stale_products(ambition);
    let line = (!dropped.is_empty()).then(|| format!("brain {account}: milestones {} left the {} ladder (catalogue revised)", dropped.join(", "), ambition.kind));
    Some(CatalogDrop { dropped, line })
}

/// What the supply half of a sync did: its line, and whether the goal of a
/// dropped milestone is to be closed (the orchestrator closes it and draws).
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogSync {
    pub lines: Vec<String>,
    pub close_running: bool,
}

/// The second half: the supply rule applies to the persisted quotas too
/// (`surplus`: the fleet's piles, `bot`: the live numbers, user rule
/// 2026-11-20). The goal one of the dropped or re-aimed milestones was
/// running closes done (no ban). `None`: nothing dropped, the life is as it
/// was.
pub fn sync_supply(life: &mut Life, drop: CatalogDrop, surplus: &[(String, f64)], bot: &BotNow) -> Option<CatalogSync> {
    let account = life.account.clone();
    let running = life.goal_origin.clone().filter(|_| !goal_closed(&life.history));
    let seed = life.seed;
    let ambition = life.ambition.as_mut()?;
    let mut dropped = drop.dropped;
    let mut lines = Vec::new();
    let revised = crate::ambition_supply::revise_supplied(ambition, bot, seed, surplus, running.as_deref());
    if !revised.is_empty() {
        lines.push(format!(
            "brain {account}: milestones {} re-aimed by the fleet's supply (piles to eat, tools someone can get): {}",
            revised.join(", "),
            crate::describe_ambition(ambition)
        ));
        dropped.extend(revised);
    }
    if dropped.is_empty() {
        return None;
    }
    let close_running = life.goal_origin.as_ref().is_some_and(|origin| dropped.contains(origin)) && !goal_closed(&life.history);
    Some(CatalogSync { lines, close_running })
}

/// After the goal of a dropped milestone closed and the draw ran: the line
/// to print; the life is normalized when a goal was drawn.
pub fn catalog_drawn(life: &mut Life, label: Option<&str>) -> String {
    match label {
        Some(label) => {
            let line = format!("brain {}: goal of a dropped milestone closed, next goal {label}", life.account);
            normalize(life);
            line
        }
        None => format!("brain {}: goal of a dropped milestone closed, nothing drawable yet", life.account),
    }
}

/// One bot's row of the A4 plan (the plan and its marker belong to the
/// orchestrator).
#[derive(Debug, Clone, PartialEq)]
pub struct RebalanceRow {
    pub from: String,
    pub kind: String,
    pub why: String,
    pub numbers: String,
}

/// The pass only concerns a life that holds an ambition.
pub fn rebalance_wants(life: &Life) -> bool {
    life.ambition.is_some()
}

/// What the bot does with its row.
#[derive(Debug, Clone, PartialEq)]
pub enum RowVerdict {
    /// No ambition held: nothing, no line.
    Nothing,
    /// Kept (operator, protected, or already on the kind) or stale: the line.
    Kept(String),
    /// The ambition is given the row's kind (the orchestrator builds it).
    Give,
}

/// Applies one row to the bot's own life (never planned, never changed:
/// protected accounts and operator ambitions; the row was planned for the
/// ambition the bot held then, a changed one makes it stale).
pub fn row_verdict(life: &Life, row: &RebalanceRow) -> RowVerdict {
    let account = &life.account;
    let Some(held) = life.ambition.as_ref() else { return RowVerdict::Nothing };
    if crate::pinned(held) || crate::maker_role(life).is_some() || crate::protected(account) {
        return RowVerdict::Kept(format!("brain rebalance {account}: kept {} (operator or protected, never touched)", held.kind));
    }
    if held.kind != row.from {
        return RowVerdict::Kept(format!("brain rebalance {account}: row stale (planned from {:?}, the bot holds {}), nothing applied", row.from, held.kind));
    }
    if held.kind == row.kind {
        return RowVerdict::Kept(format!("brain rebalance {account}: kept {} ({})", held.kind, row.numbers));
    }
    RowVerdict::Give
}

/// True when the row's kind was given.
pub fn row_applied(life: &Life, row: &RebalanceRow) -> bool {
    life.ambition.as_ref().is_some_and(|now| now.kind == row.kind)
}

/// After a daemon restart, an open `bid:` goal holds its seats again: the
/// goal must be a bid answer still open.
pub fn reengage_candidate(life: &Life) -> bool {
    life.goal_origin.as_deref().is_some_and(|origin| origin.starts_with(crate::bid_response::BID_PREFIX)) && !goal_closed(&life.history)
}

/// The lot a bid goal produces: `(item, count)`.
pub fn reengage_lot(life: &Life) -> Option<(String, i64)> {
    match &life.goal {
        GoalKind::Produce { item, count } => Some((item.clone(), *count)),
        _ => None,
    }
}
