//! The ladder's queue loop, shared by every style (`melee ladder`,
//! `ranged ladder`, `magic ladder --spell <spell>`; user 2026-11-20:
//! archers and mages climb the same camps as melee). The rung follows
//! the live levels, gear, hitpoints and holdings (`kit::ladder`),
//! re-asked after every hunt so a long run climbs.
//!
//! Deaths: an ordinary rung that killed leaves the run; a risk camp
//! (`combat_spots::risk_spot`: ice camps, demons) stays in — the death
//! is the price of its drops, what the bot dropped is destroyed (a sink
//! the fleet makes again) — and the bot re-gears at the bank before the
//! next pick, until the camp kills `RISK_DEATHS` times within
//! `RISK_WINDOW_TICKS` (`kit::survival::DeathTally`). Two deaths in a row
//! with no kill or bank trip between fail typed, any camp. Rungs that
//! yielded nothing are left out; a biting rung with no food anywhere
//! blocks typed (`Blocked: shrimp`).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{exit_code, poll_stop, CycleState, TaskSpec};
use crate::ScriptResult;

/// Run the ladder for the style the caller armed (`bot.combat_style`,
/// `bot.combat_spell`); `label` names it in the logs.
pub fn run_ladder(bot: &mut Bot, spec: &TaskSpec, no_bank: bool, label: &str) -> ScriptResult {
    // One bank visit first: the ladder reads banked food, coins and key,
    // and the gear plan wears the style's kit (bow and arrows, staff).
    if !no_bank {
        crate::kit::gear::prepare_at_bank(bot)?;
    }
    let mut tally = crate::kit::survival::DeathTally::fresh();
    let mut state = CycleState::begin(bot, spec);
    loop {
        if let Some(stop) = poll_stop(bot, &state, spec) {
            return exit_code(stop);
        }
        let (spot, plan) = crate::kit::ladder::pick(bot, tally.excluded())?;
        let plan = plan.until(spec).want(state.want_goal(bot, spec));
        bot.log(&format!("=== {} ({label} ladder, food {} required={}) ===", spot.key, plan.food_target, plan.food_required));
        bot.intend(&format!("{label} at {} (ladder)", spot.key));
        let remaining = spec.targets.count.map(|want| want.saturating_sub(state.units())).unwrap_or(0);
        let (total, died, banked) = match crate::kit::fight::run_spot_with(bot, &spot, &plan, remaining, spec.budget_max_ticks, no_bank) {
            Err(crate::ScriptError::Usage(detail)) if detail.contains("toll fence") => {
                bot.log(&format!("{} across the toll fence, re-picking without it: {detail}", spot.key));
                tally.exclude(spot.key);
                continue;
            }
            other => other?,
        };
        state.add_units(total);
        if let Some(stop) = poll_stop(bot, &state, spec) {
            return exit_code(stop);
        }
        if total > 0 || banked {
            tally.note_progress();
        }
        bot.log(&format!("ok {} killed {total}", spot.key));
        if died {
            tally.note_death(spot.key, bot.world.ticks);
            if tally.fatal() {
                return Err(crate::kit::survival::danger_error(spot.key, tally.deaths()));
            }
            if tally.accepts(spot.key) {
                // The price of a risk camp: re-gear from the bank (the
                // pick reads the worn kit), then the ladder decides again.
                bot.log(&format!("{} killed us: a risk camp, the death is its price; re-gearing at the bank", spot.key));
                if !no_bank {
                    crate::kit::gear::prepare_at_bank(bot)?;
                }
            } else {
                bot.log(&format!("{} killed us, re-picking without it", spot.key));
            }
        } else if total == 0 {
            // Every cluster failed its walk: never loop on it.
            bot.log(&format!("{} yielded nothing, re-picking without it", spot.key));
            tally.exclude(spot.key);
        }
    }
}
