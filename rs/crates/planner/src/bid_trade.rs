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

/// The origin of the current `bid:` goal when its object is off this
/// life's trade and the goal is still open (the state persisted before
/// the rule: the brain gives it back).
pub fn off_trade_answer(life: &Life) -> Option<String> {
    let origin = life.goal_origin.as_deref()?;
    let item = origin.strip_prefix(crate::bid_response::BID_PREFIX)?;
    if crate::goals::goal_closed(&life.history) || answers_trade(life, item) {
        return None;
    }
    Some(origin.to_string())
}

#[cfg(test)]
#[path = "bid_trade_tests.rs"]
mod tests;
