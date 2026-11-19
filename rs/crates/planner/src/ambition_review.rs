//! Decision D3 in the life files (user 2026-11-19, docs/plan-equilibre.txt
//! step 6): every ambition drawn for a bot outside the exceptions follows
//! the NEED alone. Pure rules; the brain (`brain_ambition`) reads the
//! fleet, stamps, logs and writes.
//!
//! - Mark: every drawn ambition records the rule it followed
//!   (`Ambition::draw_rule`, `need` or `level`); lives drawn before the
//!   mark carry none.
//! - Review (one-time): an unmarked ambition of a need-led bot is redrawn
//!   by need at its next brain pass, spread over `REVIEW_SPREAD_TICKS`
//!   after the daemon start (`review_slot_ticks`).
//! - Ladder end: a need-led bot redraws by need, never extends the same
//!   ambition; the exceptions and the operator pins keep the extension.
//! - A changed ambition replaces the goal an earlier ambition drew
//!   (`goal_from_earlier_ambition`); a side, bid or alchemy goal runs on.

use crate::ambition::{last_change_secs, pinned, Ambition};
use crate::ambition_fleet::{level_led, FleetBot};
use crate::goals::goal_closed;
use crate::model::Life;

/// `draw_rule` of an ambition drawn by need (decision D3).
pub const NEED_RULE: &str = "need";
/// `draw_rule` of an ambition drawn level-led (the D3 exceptions).
pub const LEVEL_RULE: &str = "level";
/// Ticks after the daemon start over which the one-time review spreads
/// (12 min at 60 ms dev; a bot busy at its slot is reviewed at its next
/// idle pass, one work slice later at most).
pub const REVIEW_SPREAD_TICKS: u64 = 12_000;
/// Cause written on the nodes of a goal an earlier ambition drew.
pub const STALE_GOAL_CAUSE: &str = "ambition changed: goal of an earlier ambition replaced";
/// `draw_rule` of an ambition an operator constructor set (alchemist
/// switch, overall start, market maker, skill pin): never reviewed.
pub const OPERATOR_RULE: &str = "operator";

/// True for a `why` of the need-led draw (`need`, `need:<item>`).
fn need_why(why: &str) -> bool {
    why == NEED_RULE || why.starts_with("need:")
}

/// An unmarked ambition drawn by need before the mark existed (D3 draws
/// since 03a27a2, `why` `need` or `need:<item>`) takes the `need` mark
/// instead of a redraw. True when it changed (save the life).
pub fn adopt_need_mark(ambition: &mut Ambition) -> bool {
    if !ambition.draw_rule.is_empty() || !need_why(&ambition.why) {
        return false;
    }
    ambition.draw_rule = NEED_RULE.to_string();
    true
}

/// Coverage gaps a review or ladder-end redraw of a `kind` holder offers
/// (`others`: the kinds the other fleet bots hold): the sole holder of
/// its primary group keeps it (its group alone, `ambition_fleet` rule:
/// the sole holder never leaves its group); a redundant bot, or one with
/// no group, sees the fleet's open gaps, itself included.
pub fn redraw_gaps(kind: &str, others: &[&str]) -> Vec<String> {
    if let Some(group) = crate::ambition_catalog::primary_group(kind) {
        if !others.iter().any(|other| crate::ambition_catalog::primary_group(other) == Some(group)) {
            return vec![group.to_string()];
        }
    }
    let mut all: Vec<&str> = others.to_vec();
    all.push(kind);
    crate::ambition_fleet::coverage_gaps(&all)
}

/// History tag of a drawn goal (the brain writes `goal: next:<label>`).
pub const NEXT_TAG: &str = "next:";

/// The rule a draw for this bot follows: `level` for the D3 exceptions
/// (`ambition_fleet::level_led`), `need` for every other bot.
pub fn draw_rule_for(bot: &FleetBot) -> &'static str {
    if level_led(bot) {
        LEVEL_RULE
    } else {
        NEED_RULE
    }
}

/// Stamp the rule this bot's draw followed on a freshly built ambition.
pub fn mark_rule(ambition: &mut Ambition, bot: &FleetBot) {
    ambition.draw_rule = draw_rule_for(bot).to_string();
}

/// True when the ambition must be redrawn by need now: a need-led bot
/// whose ambition carries no `need` mark (drawn by the old rule, or before
/// the mark). Never an exception account, an operator ambition (`pinned`:
/// skill pin, overall, alchemist, market maker; or marked `operator` by its
/// constructor: ryan_mason's alcher stays), nor a need draw made before
/// the mark (`need`/`need:*`, `adopt_need_mark`). An unmarked generalist
/// is reviewed.
pub fn needs_review(ambition: &Ambition, bot: &FleetBot) -> bool {
    let operator = pinned(ambition) || ambition.draw_rule == OPERATOR_RULE;
    let by_need = ambition.draw_rule == NEED_RULE || (ambition.draw_rule.is_empty() && need_why(&ambition.why));
    !level_led(bot) && !operator && !by_need
}

/// Ticks after the daemon start at which this account's review is due:
/// a stable hash of the name spread over `REVIEW_SPREAD_TICKS`.
pub fn review_slot_ticks(account: &str) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in account.to_lowercase().bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3);
    }
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    hash ^= hash >> 33;
    hash % REVIEW_SPREAD_TICKS
}

/// True when an achieved ladder extends the same ambition (the D3
/// exceptions, an operator pin); false: the bot redraws by need, which
/// may give the same kind again when the need says so.
pub fn keeps_ladder_end(ambition: &Ambition, bot: &FleetBot) -> bool {
    level_led(bot) || pinned(ambition)
}

/// True for a milestone link (`m<k>`); side, bid and alchemy links carry
/// a `prefix:`.
pub fn is_milestone_origin(origin: &str) -> bool {
    origin.strip_prefix('m').is_some_and(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The origin a `next:` label names (`produce gold_ore 74 (bid:gold_ore)`
/// -> `bid:gold_ore`): side, bid and alchemy goals carry theirs.
fn labelled_origin(label: &str) -> Option<&str> {
    let inner = label.strip_suffix(')')?;
    let (_, origin) = inner.rsplit_once(" (")?;
    origin.contains(':').then_some(origin)
}

/// True when the open goal was drawn by an earlier ambition (decision D3:
/// a changed ambition replaces it): the goal is still open, linked to no
/// milestone of the current ambition nor to a side, bid or alchemy draw
/// (`revise` drops only milestone links), and its `next:` entry is older
/// than the ambition's last change (mila_wade: « woodcutting 95 » drawn
/// by the woodcutter, run on 2.5 h after the switch to fisher). Holds for
/// a ladderless ambition too (a bot redrawn to generalist): its own
/// unlinked draws come after its change.
pub fn goal_from_earlier_ambition(life: &Life) -> bool {
    let Some(ambition) = life.ambition.as_ref() else { return false };
    if goal_closed(&life.history) {
        return false;
    }
    if let Some(origin) = life.goal_origin.as_deref() {
        let linked = ambition.milestones.iter().any(|stone| stone.id == origin);
        if linked || !is_milestone_origin(origin) {
            return false;
        }
    }
    let Some(drawn) = life.history.iter().rev().find(|entry| entry.node == "goal" && entry.outcome.starts_with(NEXT_TAG)) else { return false };
    let label = &drawn.outcome[NEXT_TAG.len()..];
    labelled_origin(label).is_none() && drawn.started < last_change_secs(ambition)
}

#[cfg(test)]
#[path = "ambition_review_tests.rs"]
mod tests;
