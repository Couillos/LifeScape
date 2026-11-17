#![forbid(unsafe_code)]
//! Food a foodless fight obtains (user 2026-11-17: "le bot prend la
//! meilleure nourriture qu'il peut s'offrir"). A hunt that bites blocks
//! typed on `melee_ladder::food_to_ask` (the biggest heal fitting half
//! its max hitpoints: swordfish from 28 HP). The fold retargets that need
//! to the best food the bot's coins buy a full load of at the market
//! (`melee_ladder::affordable_food`: best ask, else the current price),
//! never above the fitted heal; nothing priced and affordable keeps the
//! asked food (the obtain produces it or rests a bid). Live before: every
//! fighter from 28 HP asked swordfish, nobody sold any (raw swordfish
//! fishing quarantined), the fighters waited on unfilled bids.

use crate::jobs::Job;

/// Retarget one blocked need: a food the wallet covers instead of the
/// fitted one when the fitted one is out of reach.
pub fn retarget_food(need: &str, coins: i64, price_of: &dyn Fn(&str) -> Option<i64>) -> String {
    if lifescape_gamedata::food::heal(need) <= 0 {
        return need.to_string();
    }
    lifescape_gamedata::melee_ladder::affordable_food(need, lifescape_planner::replan::FOOD_BATCH, coins, price_of)
}

/// The needs of a fight's `Blocked` report, food retargeted to what the
/// bot can afford (other methods and needs untouched).
pub(crate) fn afford_food_needs(job: &Job, method: &str, missing: Vec<String>) -> Vec<String> {
    if !lifescape_planner::combat::is_fight_method(method) || !missing.iter().any(|need| lifescape_gamedata::food::heal(need) > 0) {
        return missing;
    }
    let coins = crate::brain_plan::holdings_count(job, "coins");
    let reader = crate::brain_bids::MarketReader::live(&job.account);
    let price_of = |food: &str| reader.view(food).and_then(|view| view.best_ask().or(view.current));
    missing
        .into_iter()
        .map(|need| {
            let chosen = retarget_food(&need, coins, &price_of);
            if chosen != need {
                crate::logln!("brain {}: food {need} out of reach ({coins} coins), obtaining {chosen}", job.account);
            }
            chosen
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::retarget_food;

    /// A 30-HP fighter asked swordfish: with 3 000 coins and lobsters at
    /// 120, lobsters; gear needs and unpriced food stay as asked.
    #[test]
    fn foodless_fighters_obtain_what_they_can_buy() {
        let prices = |food: &str| match food {
            "swordfish" => Some(900),
            "lobster" => Some(120),
            _ => None,
        };
        assert_eq!(retarget_food("swordfish", 3_000, &prices), "lobster");
        assert_eq!(retarget_food("swordfish", 20_000, &prices), "swordfish");
        assert_eq!(retarget_food("bronze_sword", 0, &prices), "bronze_sword");
        assert_eq!(retarget_food("swordfish", 0, &|_| None), "swordfish");
    }
}
