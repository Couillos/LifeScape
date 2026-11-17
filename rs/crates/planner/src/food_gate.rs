//! Food as a prerequisite of a method, and food a bot can come by
//! (user 2026-11-17, live jim_ellis: 10 HP, 29 coins, a funded axe step
//! whose `have_coins` hunted men — max hit 1, 10 % of his hitpoints,
//! food required — blocked on herring; the herring obtain could only
//! buy, blocked on 240 coins, a fresh `have_coins` hunted men again: one
//! node pair every 8 s for 25 minutes, nothing earned).
//!
//! - A method is workable NOW when it is feasible (`method_feasible`:
//!   quest, fit, tools) and no prerequisite is left to obtain first: a
//!   fight on a spot that bites this bot (`melee_ladder::
//!   spot_food_required`, the scripts' own rule) needs food in hand.
//!   Every resolver prefers a method workable now (`ready_now`); the
//!   others stay candidates when nothing is.
//! - A foodless fight's food is the biggest fitted heal the bot buys a
//!   load of, OR MAKES itself (`obtainable_food`, any edible object): a
//!   production leg (a hunt only on a spot it fights foodless: a hunt
//!   that needs food would wait on itself) whose skill gate its levels
//!   meet, whose tools it owns or buys, whose inputs it owns, buys or
//!   makes the same way; the most hitpoints per production step. A broke
//!   bot fishes and cooks.

use crate::expand::ReadyCtx;
use crate::tools::method_feasible;

/// Bot's base hitpoints (`None` offline: levels unknown, no judgment).
fn base_hitpoints(ctx: &ReadyCtx) -> Option<i64> {
    ctx.levels.iter().find(|(skill, _)| skill == "hitpoints").map(|(_, level)| i64::from(*level))
}

/// True when the bot owns something edible (pack, bank, worn).
pub fn holds_food(ctx: &ReadyCtx) -> bool {
    ctx.tools.iter().any(|name| lifescape_gamedata::food::heal(name) > 0)
}

/// The food a fight method lacks before it can run: the spot bites this
/// bot (not exempt, not harmless for its base hitpoints) and it owns no
/// food. `None`: not a fight, fed, a harmless spot, or unknown (bank
/// unseen, levels unknown).
pub fn food_short(method: &str, ctx: &ReadyCtx) -> Option<String> {
    let spot = crate::combat::fight_spot(method)?;
    let hitpoints = base_hitpoints(ctx)?;
    if !ctx.bank_known || holds_food(ctx) || !lifescape_gamedata::melee_ladder::spot_food_required(&spot, hitpoints) {
        return None;
    }
    Some(lifescape_gamedata::melee_ladder::food_to_ask(hitpoints).to_string())
}

/// True when the bot can work `method` now: feasible and nothing left
/// to obtain before the first action (food for a biting spot).
pub fn ready_now(method: &str, ctx: &ReadyCtx) -> bool {
    method_feasible(method, ctx) && food_short(method, ctx).is_none()
}

/// What a bot can put to work toward a production without anyone
/// selling it the product: its live levels and base hitpoints, what it
/// owns, the price of a purchase (`cost_of(item, units)`, `None`
/// unpriced) and its wallet.
pub struct Larder<'a> {
    pub level_of: &'a dyn Fn(&str) -> u8,
    pub hitpoints: i64,
    pub held: &'a dyn Fn(&str) -> i64,
    pub cost_of: &'a dyn Fn(&str, i64) -> Option<i64>,
    pub coins: i64,
}

impl Larder<'_> {
    fn buys(&self, item: &str, units: i64) -> bool {
        (self.cost_of)(item, units).is_some_and(|cost| cost <= self.coins)
    }

    fn owns_tool(&self, tool: &str) -> bool {
        crate::tools::tool_family(tool).iter().any(|name| (self.held)(name) > 0)
    }
}

/// Recipe levels `production_steps` follows (raw fish → cooked fish is
/// two: the catch, the cooking).
pub const PRODUCE_DEPTH: usize = 4;

/// Fewest production steps in which the bot makes `count` units of
/// `item` itself now (`None`: it cannot): a production leg (never
/// behind a quest; a hunt only where the bot fights foodless) whose skill
/// gate its levels meet, whose tools it owns or buys, and whose inputs it
/// holds, buys, or makes the same way, within `depth` recipe levels.
pub fn production_steps(item: &str, count: i64, larder: &Larder, depth: usize) -> Option<usize> {
    if depth == 0 {
        return None;
    }
    crate::obtain_legs::production_legs(item)
        .iter()
        .filter_map(|leg| {
            let biting = crate::combat::fight_spot(&leg.method).is_some_and(|spot| lifescape_gamedata::melee_ladder::spot_food_required(&spot, larder.hitpoints));
            if biting || leg.quest.is_some() {
                return None;
            }
            if let Some((skill, needed)) = &leg.skill {
                if i64::from((larder.level_of)(skill)) < *needed {
                    return None;
                }
            }
            if !leg.tools.iter().all(|tool| larder.owns_tool(tool) || larder.buys(tool, 1)) {
                return None;
            }
            let mut deepest = 0;
            for (input, per_unit) in &leg.inputs {
                let units = count.max(1).saturating_mul((*per_unit).max(1));
                if (larder.held)(input) >= units || larder.buys(input, units) {
                    continue;
                }
                deepest = deepest.max(production_steps(input, units, larder, depth - 1)?);
            }
            Some(1 + deepest)
        })
        .min()
}

/// The food a foodless fight obtains (user 2026-11-17: "un bot fauché
/// produit sa nourriture"), never above the `asked` heal (fitted to the
/// bot's hitpoints): the biggest heal of the `FOOD_LADDER` fish the
/// wallet buys `count` of at the market (`price_of`: best ask, else the
/// current price; one trip); else the edible object the bot makes
/// itself (`production_steps`) with the most hitpoints per production
/// step (each step works the whole load: the time proxy), then the
/// biggest heal. Nothing either way: the asked food stays (its obtain
/// rests a bid or earns coins). A non-ladder ask stays as asked. The
/// obtain of the chosen food then picks its cheapest way like any obtain.
pub fn obtainable_food(asked: &str, count: i64, price_of: &dyn Fn(&str) -> Option<i64>, larder: &Larder) -> String {
    let ladder = lifescape_gamedata::melee_ladder::FOOD_LADDER;
    let cap = lifescape_gamedata::food::heal(asked);
    if cap <= 0 || !ladder.contains(&asked) {
        return asked.to_string();
    }
    let bought = ladder
        .iter()
        .copied()
        .filter(|food| lifescape_gamedata::food::heal(food) <= cap)
        .filter(|food| price_of(food).is_some_and(|price| price.max(1).saturating_mul(count.max(1)) <= larder.coins))
        .max_by_key(|food| lifescape_gamedata::food::heal(food));
    if let Some(food) = bought {
        return food.to_string();
    }
    let mut best: Option<(i64, i64, String)> = None;
    for (food, heal) in lifescape_gamedata::food::all_foods() {
        if heal > cap || heal <= 0 {
            continue;
        }
        let Some(steps) = production_steps(&food, count, larder, PRODUCE_DEPTH) else { continue };
        let per_step = heal * 1000 / steps.max(1) as i64;
        if best.as_ref().is_none_or(|(known_rate, known_heal, _)| (per_step, heal) > (*known_rate, *known_heal)) {
            best = Some((per_step, heal, food));
        }
    }
    best.map(|(_, _, food)| food).unwrap_or_else(|| asked.to_string())
}

#[cfg(test)]
#[path = "food_gate_tests.rs"]
mod tests;
