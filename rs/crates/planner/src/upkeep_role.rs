#![forbid(unsafe_code)]
//! Life upkeep decisions (lot 4), second family: the roles. The market
//! maker's switch and retirement (user 2026-11-20, « makers unused ») and
//! the operator's ambition switch (user 2026-11-18, 2026-11-20, 2026-11-21).
//! Pure: the life, the facts the orchestrator read (the maker's coins and
//! resting orders, the operator's filed kind, the bot's live numbers) and
//! the clock go in; the changes of the life and the log lines come out.
//! Cancelling quotes, freeing the board and drawing goals stay with the
//! orchestrator, in the order the drive gives them.

use crate::ambition::{BotNow, Stamp};
use crate::launch_apply::push_history;
use crate::upkeep::Clock;
use crate::{abandon_for_role, become_alcher, become_quoting, maker_role, normalize, protected, Archetype, Life, MakerRole, ALCHER, OPERATOR_ROLE_CAUSE};

/// One resting order of the bot, as the rules read it.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderFact {
    pub order_id: i64,
    pub buy: bool,
    /// The item of the book the order sits on (`market_audit::book_item`).
    pub book_item: i64,
    /// A quote of the maker's own pass (motive `market_make`).
    pub market_make: bool,
    /// Open or matched with units left.
    pub live: bool,
}

/// The orders the switch withdraws: every live buy and the listings of
/// the maker's slice (reason `role`).
pub fn switch_cancels(rows: &[OrderFact], slice: &[i64]) -> Vec<i64> {
    rows.iter().filter(|row| row.live && (row.buy || slice.contains(&row.book_item))).map(|row| row.order_id).collect()
}

/// The switch written into the life (every other node abandoned, goal
/// `MakeMarket`); returns the log line.
pub fn apply_switch(life: &mut Life, role: &MakerRole, coins: i64, clock: Clock) -> String {
    let abandoned = become_quoting(life, coins, clock.now);
    push_history(life, "goal", &format!("role:market_maker quoting slice {}/{} ({coins} coins >= {})", role.slice, role.slices, role.capital_target), clock.now);
    format!(
        "brain {}: market maker slice {}/{}: capital {coins} >= target {}, quoting for good ({} nodes abandoned)",
        life.account,
        role.slice,
        role.slices,
        role.capital_target,
        abandoned.len()
    )
}

/// A maker life retires while makers are unused (`MAKERS_ACTIVE` false,
/// user 2026-11-20).
pub fn retire_due(life: &Life) -> bool {
    !crate::MAKERS_ACTIVE && maker_role(life).is_some()
}

/// The quotes a retirement cancels: its own live quotes (reason `maker_retired`).
pub fn retire_cancels(rows: &[OrderFact]) -> Vec<i64> {
    rows.iter().filter(|row| row.market_make && row.live).map(|row| row.order_id).collect()
}

/// The retirement written into the life: producer archetype back, every
/// open node abandoned, no goal origin; returns the log line. The neediest
/// trade is drawn next (the orchestrator).
pub fn apply_retire(life: &mut Life, cancelled: usize, now: i64) -> String {
    life.profile.archetype = Archetype::Producer;
    let abandoned = abandon_for_role(life, crate::MAKER_RETIRED_CAUSE);
    life.goal_origin = None;
    push_history(life, "goal", "role:market_maker retired", now);
    format!("brain {}: market maker retired ({cancelled} quotes cancelled, {} nodes abandoned), drawing the neediest trade", life.account, abandoned.len())
}

/// Kind of the operator's request that means « the natural balance ».
pub const NATURAL: &str = "natural";

/// A trade the fleet draws (catalogue kind with a ladder, never the market
/// maker): an operator may hand it out.
pub fn drawable_trade(kind: &str) -> bool {
    crate::ambition_catalog::specs().iter().any(|entry| entry.kind == kind) && !crate::ambition::ladderless(kind) && kind != crate::MARKET_MAKER_KIND
}

/// Unban the skills a role switch banned (ryan_mason 2026-11-18: its
/// abandoned nodes read as a stuck graph, magic banned 6000 ticks); returns
/// the lines, the life gets the `unban:` entries.
pub fn lift_role_bans(life: &mut Life, clock: Clock) -> Vec<String> {
    let skills = crate::role_lifted_bans(&life.history, &|stamp| clock.ticks_since(stamp));
    let mut lines = Vec::new();
    for skill in &skills {
        push_history(life, "goal", &crate::unban_entry(skill), clock.now);
        lines.push(format!("brain {}: unban:{skill} (an operator role switch caused it)", life.account));
    }
    lines
}

/// What the operator's switch asks of the orchestrator, besides the text.
#[derive(Debug, Clone, PartialEq)]
pub enum Switch {
    /// `natural`: the ambition given by value over the fleet's demand.
    Natural,
    /// A trade the fleet draws: built like a redraw over the demand.
    Trade(String),
}

/// The first decision of a filed switch.
#[derive(Debug, Clone, PartialEq)]
pub enum OperatorStep {
    /// Nothing more: the lines to print (a drop, a refusal, an unknown kind).
    Done(Vec<String>),
    /// The ambition is to be given on `draft` (a copy of the life: it
    /// replaces the life only when the giving step succeeded).
    Assign { draft: Box<Life>, switch: Switch },
    /// The alchemist: built from the bot's live numbers.
    Alcher,
}

/// A filed switch `kind` for this life (user 2026-11-18: « créé l'ambition
/// alcher et donne-la à Ryan Mason »; 2026-11-20, every drawn trade;
/// 2026-11-21: `natural`). A protected account drops it (overall for good).
pub fn operator_step(life: &Life, kind: &str) -> OperatorStep {
    if protected(&life.account) {
        return OperatorStep::Done(vec![format!("brain {}: operator ambition {kind} dropped (protected account, overall for good)", life.account)]);
    }
    if kind == NATURAL {
        // The pin (or the alchemist type) leaves; the draft carries it.
        if maker_role(life).is_some() {
            return OperatorStep::Done(vec![format!("brain {}: operator ambition natural refused (protected account or market maker)", life.account)]);
        }
        let mut draft = life.clone();
        if let Some(held) = draft.ambition.as_mut() {
            held.pin = None;
            held.draw_rule.clear();
        }
        draft.goal_origin = None;
        return OperatorStep::Assign { draft: Box::new(draft), switch: Switch::Natural };
    }
    if kind != ALCHER {
        if !drawable_trade(kind) {
            return OperatorStep::Done(Vec::new());
        }
        let mut draft = life.clone();
        draft.goal_origin = None;
        return OperatorStep::Assign { draft: Box::new(draft), switch: Switch::Trade(kind.to_string()) };
    }
    OperatorStep::Alcher
}

/// The outcome of giving the ambition on the draft.
#[derive(Debug, Clone, PartialEq)]
pub enum Assigned {
    /// Refused: the life is untouched, the line to print.
    Refused(String),
    /// Given: the life is the draft, its open work stopped; the line to
    /// print, then the next goal is drawn at once.
    Drawing(String),
}

/// The draft becomes the life when the giving step `ok`ed.
pub fn operator_assigned(life: &mut Life, draft: Life, switch: &Switch, ok: bool, now: i64) -> Assigned {
    if !ok {
        return Assigned::Refused(match switch {
            Switch::Natural => format!("brain {}: natural balance not given (no draw or change refused), life untouched", life.account),
            Switch::Trade(kind) => format!("brain {}: operator ambition {kind} not given (nothing buildable or refused), life untouched", life.account),
        });
    }
    *life = draft;
    let abandoned = abandon_for_role(life, OPERATOR_ROLE_CAUSE);
    match switch {
        Switch::Natural => {
            push_history(life, "goal", "role:natural (operator)", now);
            Assigned::Drawing(format!("brain {}: ambition -> natural balance (operator), {} nodes abandoned", life.account, abandoned.len()))
        }
        Switch::Trade(kind) => {
            push_history(life, "goal", &format!("role:{kind} (operator)"), now);
            Assigned::Drawing(format!("brain {}: ambition -> {kind} (operator), {} nodes abandoned", life.account, abandoned.len()))
        }
    }
}

/// After the first goal was drawn: the line to print (if any); the life
/// is normalized.
pub fn operator_drawn(life: &mut Life, switch: &Switch, label: Option<&str>) -> Option<String> {
    let line = label.map(|label| match switch {
        Switch::Natural => format!("brain {}: first goal {label}", life.account),
        Switch::Trade(kind) => format!("brain {}: {kind}'s first goal {label}", life.account),
    });
    normalize(life);
    line
}

/// The alchemist switch, built from the bot's live numbers (`bot`): its
/// open nodes abandoned, the ambition replaced. `Ok`: the line to print,
/// the next goal is drawn at once; `Err`: refused, the line to print.
pub fn operator_alcher(life: &mut Life, bot: &BotNow, stamp: Stamp, now: i64) -> Result<String, String> {
    match become_alcher(life, bot, stamp) {
        Ok(abandoned) => {
            push_history(life, "goal", "role:alcher (operator)", now);
            Ok(format!("brain {}: ambition -> alcher (operator), {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(",")))
        }
        Err(error) => Err(format!("brain {}: operator ambition alcher refused: {error:?}", life.account)),
    }
}

/// After the alchemist's first goal was drawn (alchemy first, else money).
pub fn alcher_drawn(life: &mut Life, label: Option<&str>) -> String {
    let line = match label {
        Some(label) => format!("brain {}: alcher's first goal {label}", life.account),
        None => format!("brain {}: alcher drew no goal yet (see the reason above)", life.account),
    };
    normalize(life);
    line
}
