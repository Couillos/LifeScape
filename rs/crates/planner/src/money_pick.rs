//! Money and coin nodes choosing from the money guide (plan rendements
//! R7, D10, D14, D17): the brain hands the guide's lines the bot may take
//! (`ReadyCtx::money_guide`: the precomputed guide's rows mapped to the
//! planner methods they launch, judged by `guide_feasible::feasible_for`
//! on the bot's live state, weighed by its profile); the resolver keeps
//! the planner's own gates and draws (`guide_pick`).
//!
//! Classes, the first non-empty one draws (`guide_pick::class_order`):
//! - 0: workable NOW (`food_gate::ready_now`: tools held, quest, fit,
//!   food in hand where the spot bites; a trade lot: `trade_craft::
//!   workable`) and feasible in the guide;
//! - 1: feasible with a prerequisite the bot gets on its own (food for a
//!   biting spot, a lot's coins the guide still counts);
//! - 2: a tool to obtain first (the node blocks on it, `missing:<tool>`);
//! - never: quarantined or cooled (`ReadyCtx::sick`), a quest missing, a
//!   spot its stats do not survive, a trade lot that fits no wallet, a
//!   method a dependency cycle excluded.
//! The garde-fous stay: the trade lot whose inputs were bought is crafted
//! first (`trade_craft::sticky`, before this), and a node that blocked on
//! a tool for its previous pick (`blocked_by`) takes that pick again once
//! it works now (`Choice::sticky`), never a fresh draw that strands the
//! tool. No drawable line: `false`, the static money methods rank.

use crate::expand::ReadyCtx;
use crate::guide_pick::{draw, first_class, Candidate, Choice, Drawn, GuideLine};
use crate::model::{Alternative, Node, NodeStatus};
use crate::tools::{method_feasible, required_tool};

/// The class of one line for this bot (module doc), `None`: never drawn.
pub fn class_of(line: &GuideLine, ctx: &ReadyCtx) -> Option<u8> {
    let method = line.method.as_str();
    if crate::trade_craft::is_trade(method) {
        return crate::trade_craft::workable(method, ctx).then_some(if line.feasible { 0 } else { 1 });
    }
    let guide = if line.feasible { 0 } else { 1 };
    if crate::food_gate::ready_now(method, ctx) {
        return Some(guide);
    }
    if method_feasible(method, ctx) {
        return Some(1);
    }
    let tool_short = ctx.sick(method).is_none() && crate::quests::missing_quest(method, None, ctx).is_none() && crate::combat::drop_farm_fit(method, ctx) && required_tool(method).and_then(|need| need.missing(ctx)).is_some();
    tool_short.then_some(2)
}

/// Tick-equivalent of a profit (the alternatives' order, as before R7).
fn est_ticks(profit: f64) -> u64 {
    if profit > 0.0 {
        (6000.0 * 1000.0 / (profit + 1000.0)) as u64
    } else {
        6000
    }
}

/// Resolves a money or coin node from `ctx.money_guide` (module doc);
/// `false` when no line is drawable (the caller ranks the static money
/// methods). The choice is left in `ctx.drawn`.
pub(crate) fn resolve_from_guide(node: &mut Node, ctx: &ReadyCtx) -> bool {
    let lines: Vec<&GuideLine> = ctx.money_guide.iter().filter(|line| !node.excluded.contains(&line.method)).collect();
    let classes: Vec<u8> = lines.iter().map(|line| class_of(line, ctx).unwrap_or(u8::MAX)).collect();
    let (Some(class), members) = first_class(&classes) else { return false };
    let previous = node.blocked_by.clone();
    let again = previous.as_deref().and_then(|method| members.iter().copied().find(|index| class == 0 && lines[*index].method == method));
    let (chosen, drawn, sticky) = match again {
        Some(index) => (index, Drawn { index: 0, band: 1, candidates: members.len(), explored: false }, true),
        None => {
            let candidates: Vec<Candidate> = members.iter().map(|index| Candidate { score: lines[*index].profit_6000, weight: lines[*index].weight, runs: lines[*index].runs }).collect();
            let Some(drawn) = draw(&candidates, ctx.draw_seed) else { return false };
            (members[drawn.index], drawn, false)
        }
    };
    let line = lines[chosen];
    // A recipe on its own stock runs its lot (`stock_lot`, `--use`); a
    // trade lot sets its own (`trade_craft::lock`).
    node.batch = line.lot.max(0);
    let mut ranked: Vec<(u8, Alternative)> = lines
        .iter()
        .zip(&classes)
        .map(|(line, class)| (*class, Alternative { method: line.method.clone(), est_ticks: est_ticks(line.profit_6000), est_coins: 0 }))
        .collect();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.est_ticks.cmp(&right.1.est_ticks)));
    node.alternatives = ranked.into_iter().map(|(_, alt)| alt).collect();
    if class < 2 {
        node.method = Some(line.method.clone());
        crate::trade_craft::lock(node, ctx);
    } else {
        // The drawn line needs a tool first: the obtain chain funds it,
        // the method kept (the block's `blocked_by`: taken again once the
        // tool is held).
        node.method = Some(line.method.clone());
        if let Some(tool) = required_tool(&line.method).and_then(|need| need.missing(ctx)) {
            node.status = NodeStatus::Blocked { reason: format!("missing:{tool}") };
        }
    }
    *ctx.drawn.borrow_mut() = Some(Choice { kind: "money", method: line.method.clone(), score: line.profit_6000, class, drawn, sticky });
    true
}

#[cfg(test)]
#[path = "money_pick_tests.rs"]
mod tests;
