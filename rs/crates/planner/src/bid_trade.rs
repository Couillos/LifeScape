//! A bot answers the market in its own trade (user 2026-11-20, D3: « je
//! t'avais demandé de revoir les ambitions » ; « il faut des acheteurs
//! pour écouler les marchandises »). Live 00:15 -> 01:15 UTC after the
//! ambition review: 60 of the 63 bots outside the market makers held a
//! `bid:` goal (a lot worth more coins per tick than the bot's measured
//! gain, D1, 0.1 to 1 coin/tick, so nearly every lot won the draw):
//! runecrafters made mithril bars, priests mithril battleaxes, cooks water
//! runes; cooking, prayer and firemaking 0 % of the fleet's time, mining,
//! smelting and smithing 51 %. The need-drawn ambition never ran.
//!
//! Rule: a bot whose ambition follows the need answers only the lots its
//! kind's primary skill makes (`ambition_catalog::serves` = 1: its ladder
//! lists the object or its primary skill makes it in one step); a smith
//! answers bars and armour, a miner ore, a cook cooked food, a
//! runecrafter runes. A kind whose skill makes nothing (priest, firemaker,
//! ranger, warrior) answers no lot: it trains, and its training BUYS what
//! it burns (bones, logs, arrows, runes). The D3 exceptions keep the whole
//! market: `LEVEL_LED_ACCOUNTS`, the kinds without a trade (market maker,
//! overall, generalist, alchemist), a life without ambition. Arbitrage
//! with the shops (D2, `merchant_arbitrage`) is another path, open to all.
//!
//! While waiting (user 2026-11-20, « oui implémente ça » : the hybrid):
//! measured 01:36 -> 02:36 UTC after the rule, the consumers bid for what
//! they burn (56 000 raw lobsters, 23 000 big bones, 39 000 willow logs)
//! and waited, fillers 37 % of the fleet's time (16 % before), the
//! transformation 5 % (29 %). A bot whose ambition goal waits on a buy
//! answers the WHOLE market meanwhile: the brain sets the milestone goal
//! aside (milestone untouched, the buy left on the book) and draws an
//! answer of any trade, recorded `WAIT_ANSWER_TAG`; such an answer is
//! never given back as off the trade; the draw after it returns to the
//! ambition.
//! Pure: game data and the life only.

use crate::model::Life;

/// True when this life may answer a lot of `item` (module doc).
pub fn answers_trade(life: &Life, item: &str) -> bool {
    let Some(ambition) = life.ambition.as_ref() else { return true };
    let exception = crate::ambition_fleet::LEVEL_LED_ACCOUNTS.iter().any(|name| name.eq_ignore_ascii_case(&life.account)) || crate::market_maker::maker_role(life).is_some();
    if exception || crate::ambition_catalog::primary_group(&ambition.kind).is_none() {
        return true;
    }
    crate::ambition_catalog::serves(&ambition.kind, item) >= 1.0
}

/// History outcome (node `goal`) that follows the draw of an answer made
/// while the ambition waits on a buy: `wait_answer:<origin>`.
pub const WAIT_ANSWER_TAG: &str = "wait_answer:";

/// True when the current goal was drawn as an answer while waiting (a
/// `WAIT_ANSWER_TAG` entry after the latest goal draw).
pub fn drawn_while_waiting(history: &[crate::model::HistoryEntry]) -> bool {
    for entry in history.iter().rev().filter(|entry| entry.node == "goal") {
        if entry.outcome.starts_with(WAIT_ANSWER_TAG) {
            return true;
        }
        if entry.outcome.starts_with(crate::ambition_review::NEXT_TAG) {
            return false;
        }
    }
    false
}

/// Credits a production milestone with what its goal made before being
/// set aside (review 2026-11-20: the redraw asked the whole count again):
/// the goal's quota node (`ObtainItems` of the goal's object, at or above
/// the floor) counts the units made since the draw, added to the
/// milestone's progress (at
/// most its count less one: the close marks it done). Returns the units
/// credited.
pub fn credit_made(life: &mut Life, milestone: &str) -> i64 {
    let crate::model::GoalKind::Produce { item, count: quota } = &life.goal else { return 0 };
    let floor = life.memory.floor.unwrap_or(0);
    // The quota node starts at the stock held at the draw (its target the
    // quota plus that stock, `append_goal_with`): only what it made since.
    let made = life
        .nodes
        .iter()
        .filter(|node| crate::lifecycle_retire::node_number(&node.id).is_some_and(|number| number >= floor))
        .filter(|node| matches!(&node.kind, crate::model::NodeKind::ObtainItems { item: wanted, .. } if wanted == item))
        .map(|node| node.progress_current - (node.progress_target - quota).max(0))
        .max()
        .unwrap_or(0)
        .max(0);
    let Some(stone) = life.ambition.as_mut().and_then(|held| held.milestones.iter_mut().find(|stone| stone.id == milestone)) else { return 0 };
    let crate::ambition::Target::Produce { count, .. } = stone.target else { return 0 };
    let before = stone.progress.0;
    stone.progress = ((before + made).min(count - 1).max(before), count);
    stone.progress.0 - before
}

/// The origin of the current `bid:` goal when its object is off this
/// life's trade and the goal is still open (the state persisted before
/// the rule: the brain gives it back).
pub fn off_trade_answer(life: &Life) -> Option<String> {
    let origin = life.goal_origin.as_deref()?;
    let item = origin.strip_prefix(crate::bid_response::BID_PREFIX)?;
    if crate::goals::goal_closed(&life.history) || drawn_while_waiting(&life.history) || answers_trade(life, item) {
        return None;
    }
    Some(origin.to_string())
}

#[cfg(test)]
#[path = "bid_trade_tests.rs"]
mod tests;
