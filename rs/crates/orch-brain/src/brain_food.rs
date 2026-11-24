#![forbid(unsafe_code)]
//! Food a foodless fight obtains (user 2026-11-17: "le bot prend la
//! meilleure nourriture qu'il peut s'offrir", then "un bot fauché produit
//! sa nourriture"). A hunt that bites blocks typed on
//! `melee_ladder::food_to_ask` (the biggest heal fitting half its max
//! hitpoints: swordfish from 28 HP). The fold retargets that need
//! (`planner::food_gate::obtainable_food`): the best fitted fish the
//! bot's coins buy a full load of at the market (best ask, else the
//! current price); else the food it makes itself — levels, owned or
//! affordable tools and inputs, the most hitpoints per production step
//! (shrimps or chicken for a 10-HP bot at fishing and cooking 1); else
//! the asked food stays (its obtain rests a bid or earns coins). Live
//! before: every fighter from 28 HP asked swordfish nobody sold; then
//! jim_ellis (29 coins) asked herring he could neither buy nor make, its
//! coins came from the very hunt that needed it.


use crate::jobs::Job;

/// Retarget one blocked need: a food the bot buys or makes instead of
/// the fitted one when the fitted one is out of reach.
pub fn retarget_food(need: &str, price_of: &dyn Fn(&str) -> Option<i64>, larder: &lifescape_planner::food_gate::Larder) -> String {
    if lifescape_gamedata::food::heal(need) <= 0 {
        return need.to_string();
    }
    lifescape_planner::food_gate::obtainable_food(need, lifescape_planner::replan::FOOD_BATCH, price_of, larder)
}

/// The needs of a fight's `Blocked` report, food retargeted to what the
/// bot can buy or make (other methods and needs untouched).
pub(crate) fn afford_food_needs(job: &Job, method: &str, missing: Vec<String>) -> Vec<String> {
    if !lifescape_planner::combat::is_fight_method(method) || !missing.iter().any(|need| lifescape_gamedata::food::heal(need) > 0) {
        return missing;
    }
    let coins = crate::brain_plan::holdings_count(job, "coins");
    let reader = crate::brain_bids::MarketReader::live(&job.account);
    let price_of = |item: &str| reader.view(item).and_then(|view| view.best_ask().or(view.current));
    let live = job.inner.try_lock().map(|inner| inner.live_or_null()).unwrap_or_else(|_| crate::jobs::null_live());
    let levels = crate::brain_market::live_levels(&live);
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    // Owned stacks (pack, bank) and worn gear, read once.
    let mut owned = crate::brain_plan::live_stacks(job);
    for tool in crate::brain_plan::carried_tools(job) {
        if !owned.iter().any(|(name, _)| name.eq_ignore_ascii_case(&tool)) {
            owned.push((tool, 1));
        }
    }
    let held = |item: &str| owned.iter().filter(|(name, _)| name.eq_ignore_ascii_case(item)).map(|(_, count)| *count).sum::<i64>();
    // A purchase: the cheapest of the keeper and the market.
    let cost_of = |item: &str, units: i64| {
        let unit = [lifescape_gamedata::obtain::merchant_price(item), price_of(item)].into_iter().flatten().min()?;
        Some(unit.max(1).saturating_mul(units.max(1)))
    };
    let larder = lifescape_planner::food_gate::Larder { level_of: &level_of, hitpoints: i64::from(level_of("hitpoints")), held: &held, cost_of: &cost_of, coins };
    missing
        .into_iter()
        .map(|need| {
            let chosen = retarget_food(&need, &price_of, &larder);
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
    use lifescape_planner::food_gate::Larder;

    /// A 30-HP fighter asked swordfish: with 3 000 coins and lobsters at
    /// 120, lobsters; 20 000 coins, swordfish; gear needs stay as asked.
    #[test]
    fn foodless_fighters_obtain_what_they_can_buy() {
        let prices = |food: &str| match food {
            "swordfish" => Some(900),
            "lobster" => Some(120),
            _ => None,
        };
        let cost_of = |_: &str, _: i64| None;
        let wallet = Larder { level_of: &|_| 1, hitpoints: 30, held: &|_| 0, cost_of: &cost_of, coins: 3_000 };
        assert_eq!(retarget_food("swordfish", &prices, &wallet), "lobster");
        let rich = Larder { coins: 20_000, ..wallet };
        assert_eq!(retarget_food("swordfish", &prices, &rich), "swordfish");
        assert_eq!(retarget_food("bronze_sword", &prices, &rich), "bronze_sword");
    }

    /// jim_ellis (10 HP, 29 coins, fishing and cooking 1): herring at 12
    /// is out of reach, the food he makes himself is obtained instead.
    #[test]
    fn a_broke_fighter_obtains_the_food_it_makes() {
        let prices = |food: &str| (food == "herring").then_some(12);
        let cost_of = |item: &str, units: i64| (item == "net").then_some(5 * units);
        let larder = Larder { level_of: &|_| 1, hitpoints: 10, held: &|_| 0, cost_of: &cost_of, coins: 29 };
        let chosen = retarget_food("herring", &prices, &larder);
        assert_ne!(chosen, "herring");
        assert_eq!(lifescape_gamedata::food::heal(&chosen), 3, "{chosen}");
    }
}
