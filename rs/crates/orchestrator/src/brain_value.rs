#![forbid(unsafe_code)]
//! A4 in the brain (plan progression, user 2026-11-21): the ambition the
//! brain GIVES, by the value of the unserved demand
//! (`lifescape_planner::ambition_value`). This file only plugs the live
//! inputs into the pure rules: the market's bids and prices
//! (`market_demand::ValueInputs`), the measured work rates
//! (`brain_plan_read::live_rate`), the best XP rates of the guides'
//! catalogue (`investment::best_xp_rate`).
//!
//! - `with_context`: a `ValueContext` over the inputs, for one body;
//! - `draw`: the kind a bot is given: the level-led exceptions and the
//!   market makers keep the draw of D3 (`draw_kind`), every other bot the
//!   best score (`pick_by_value`), the numbers logged.

use lifescape_planner::investment::best_xp_rate;
use lifescape_planner::{draw_kind, level_led, pick_by_value, FleetBot, KindDemand, ValueContext};

use crate::market_demand::ValueInputs;

/// Runs `body` with a `ValueContext` over `inputs` and the fleet's
/// `surplus` (the piles the training of a consumer skill burns).
pub(crate) fn with_context<T>(inputs: &ValueInputs, surplus: &[(String, f64)], body: impl FnOnce(&ValueContext) -> T) -> T {
    let rate = crate::brain_plan_read::live_rate();
    let price = |item: &str| inputs.price_of(item);
    let context = ValueContext { bids: &inputs.bids, alch_offers: &inputs.alch_offers, surplus, price: &price, xp_rate: &best_xp_rate, rate: &rate };
    body(&context)
}

/// The kind `bot` is given and its `why` (A4): a bot of the D3 exceptions
/// or a market maker keeps its level-led draw over `gaps` and `weights`;
/// any other the best value score given the kinds `held` by the others.
/// `exclude`: kinds kept out (the one just refused). Logs the numbers of
/// a value pick.
pub(crate) fn draw(account: &str, bot: &FleetBot, gaps: &[String], weights: &[KindDemand], inputs: &ValueInputs, surplus: &[(String, f64)], held: &[String], exclude: &[&str]) -> (String, String) {
    if level_led(bot) {
        return draw_kind(bot, gaps, weights, exclude);
    }
    let refs: Vec<&str> = held.iter().map(String::as_str).collect();
    match with_context(inputs, surplus, |context| pick_by_value(bot, &refs, context, exclude)) {
        Some(pick) => {
            crate::logln!("brain {account}: ambition {} given by value ({})", pick.kind, pick.numbers());
            (pick.kind.clone(), pick.why())
        }
        // Nothing drawable (every kind excluded): the draw's own end, the
        // generalist, as `draw_kind` gives it.
        None => draw_kind(bot, gaps, weights, exclude),
    }
}

/// Seconds the fleet's value inputs are reused (a climb launches rarely;
/// the book is read once a minute at most).
const INPUTS_REUSE_SECS: u64 = 60;

/// The fleet's value inputs and surplus now, reused `INPUTS_REUSE_SECS`.
fn fleet_inputs() -> (ValueInputs, Vec<(String, f64)>) {
    type Cache = Option<(std::time::Instant, ValueInputs, Vec<(String, f64)>)>;
    static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((read_at, inputs, surplus)) = cache.as_ref() {
        if read_at.elapsed().as_secs() < INPUTS_REUSE_SECS {
            return (inputs.clone(), surplus.clone());
        }
    }
    let demand = crate::market_demand::kind_demand();
    *cache = Some((std::time::Instant::now(), demand.value.clone(), demand.surplus.clone()));
    (demand.value, demand.surplus)
}

/// The climb that opens a lucrative way pulls (A1 (a)): the investments
/// the market's unserved demand offers that need `skill` above the bot's
/// level (`investment` over the skill's own gates), the alchemy for
/// magic; `lifescape_planner::investment::time_value` turns them into the
/// coins a tick of the bot's time is worth. Empty when nothing pays.
pub(crate) fn pending_investments(skill: &str, level_of: &dyn Fn(&str) -> u8) -> Vec<lifescape_planner::investment::Investment> {
    let (inputs, surplus) = fleet_inputs();
    with_context(&inputs, &surplus, |context| {
        let own = [skill];
        let mut out: Vec<lifescape_planner::investment::Investment> = inputs
            .bids
            .iter()
            .filter(|bid| bid.unit_price > 0.0 && bid.unmet > 0)
            .filter_map(|bid| lifescape_planner::investment::investment(&bid.item, bid.unit_price, bid.unmet as f64, &own, level_of, context.price, context.xp_rate, context.rate))
            .filter(|found| found.gates.iter().any(|gate| gate.skill == skill && gate.climb_ticks > 0.0))
            .collect();
        if skill == "magic" {
            out.extend(lifescape_planner::investment::alchemy_investment(level_of, context.alch_offers, context.price, context.xp_rate).filter(|found| found.climb_ticks > 0.0));
        }
        out
    })
}
