#![forbid(unsafe_code)]
//! What a resolve leaves in the life's memory (lot 3 of the launch
//! refactor, moved from `brain_picks::record` and `brain_trade::note_lot`):
//! the choice drawn from a guide (R7) and the 100 % market lot it locked
//! (R5). Pure over the memory: the log lines and the order-board claim
//! come back as `Emit`s for the orchestrator to perform.

use crate::expand::ReadyCtx;
use crate::launch_decide::Emit;
use crate::life_memory::PickRecord;
use crate::{LifeMemory, Node, NodeKind};

/// Records the choice the resolve of `node` made, when the node kept it
/// (a ban steering may move a money node off it: logged, not recorded).
/// `filler`: a filler window's coin node (outside the graph), kept under
/// the kind `filler` in its own pool. True when the memory changed.
pub fn record_choice(memory: &mut LifeMemory, account: &str, node: &Node, ctx: &ReadyCtx, now: i64, filler: bool, emits: &mut Vec<Emit>) -> bool {
    let Some(mut choice) = ctx.drawn.borrow_mut().take() else { return false };
    if filler {
        choice.kind = crate::life_memory::FILLER_PICK_KIND;
    }
    let skill = match &node.kind {
        NodeKind::ReachLevel { skill, .. } => skill.clone(),
        _ => String::new(),
    };
    if node.method.as_deref() != Some(choice.method.as_str()) {
        emits.push(Emit::Log(format!("brain {account}: {} {} pick {} not kept (now {:?})", node.id, choice.kind, choice.method, node.method)));
        return false;
    }
    emits.push(Emit::Log(format!(
        "brain {account}: {} {} pick {}{} (score {:.3}, class {}, {} in band of {}{}{})",
        node.id,
        choice.kind,
        choice.method,
        if skill.is_empty() { String::new() } else { format!(" for {skill}") },
        choice.score,
        choice.class,
        choice.drawn.band,
        choice.drawn.candidates,
        if choice.drawn.explored { ", exploration" } else { "" },
        if choice.sticky { ", previous pick again" } else { "" }
    )));
    memory.note_pick(PickRecord {
        at: now,
        node: node.id.clone(),
        kind: choice.kind.to_string(),
        skill,
        method: choice.method.clone(),
        score: choice.score,
        class: choice.class,
        band: choice.drawn.band,
        candidates: choice.drawn.candidates,
        explored: choice.drawn.explored,
        sticky: choice.sticky,
    });
    true
}

/// `record_choice` for a graph node.
pub fn record_pick(memory: &mut LifeMemory, account: &str, node: &Node, ctx: &ReadyCtx, now: i64, emits: &mut Vec<Emit>) -> bool {
    record_choice(memory, account, node, ctx, now, false, emits)
}

/// Records the lot the resolve just locked (idempotent per node): the
/// claim on the order board, then the log line. `lot_claim` stays an
/// effect (`Emit::ClaimLot`). True when a lot was opened.
pub fn note_lot(memory: &mut LifeMemory, account: &str, node: &Node, ctx: &ReadyCtx, now: i64, emits: &mut Vec<Emit>) -> bool {
    let Some(lot) = crate::trade_craft::open_lot(&mut memory.trades, node, ctx, now) else { return false };
    emits.push(Emit::ClaimLot(lot.clone()));
    let buys: Vec<String> = lot.buys.iter().map(|(item, units)| format!("{units} {item}")).collect();
    emits.push(Emit::Log(format!(
        "brain {account}: {} trade lot {} x{} (bound {}): buys {} for {}, predicted {:+} (guide {:.0} / 6000)",
        node.id,
        lot.method,
        lot.lot,
        lot.bound,
        if buys.is_empty() { "nothing (held)".to_string() } else { buys.join(", ") },
        lot.cost,
        lot.predicted,
        crate::trade_craft::offer_in(ctx, &lot.method).map(|offer| offer.profit_6000).unwrap_or(0.0)
    )));
    true
}
