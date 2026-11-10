//! Melee activity: best F2P spot for the level, best banked gear,
//! banked food. The queue loop only: spot picking, death tally and the
//! kill loop live in `kit::fight`, gear in `kit::gear`, survival in
//! `kit::survival`. Stop conditions come from `kit::cycle` (count and
//! budget polls at the top of every queue turn).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{exit_code, poll_stop, CycleState};
use crate::kit::survival::check_events;
use crate::ScriptResult;

/// Melee farmer: `melee [spot ...] [--kills N] [--ticks N] [--no-bank]`.
pub fn run_melee(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("melee", args)?;
    let spots = spec.resource.clone();
    let no_bank = spec.no_bank;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    crate::kit::survival::wait_pack_streamed(bot)?;
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && (!bot.world.stats.contains_key(&0) || !bot.world.stats.contains_key(&3)) {
        bot.pump(5)?;
    }
    check_events(bot)?;
    let picked: Vec<knowledge::combat_spots::CombatSpot> = {
        let named: Vec<_> = spots.iter().filter_map(|key| knowledge::combat_spots::by_key(key)).collect();
        if named.is_empty() {
            vec![crate::kit::fight::auto_spot(bot)]
        } else {
            named
        }
    };
    bot.log(&format!("melee={} spots={}", crate::kit::gear::melee_level(bot), picked.iter().map(|spot| spot.key).collect::<Vec<_>>().join(",")));
    // Deaths re-pick, never identically: the killer spot is excluded for
    // the run (one death hunts the fallback or the next picked spot, two
    // consecutive deaths fail typed). Kills and bank trips reset the
    // streak: only back-to-back deaths with no progress between are fatal.
    let mut queue = picked;
    let mut tally = crate::kit::survival::DeathTally::fresh();
    // Auto mode (no explicit spot): the pick is a suggestion, never an
    // order. Unpaid and across the toll fence, it re-picks walkable
    // instead of ending the run idle; explicit picks keep the Usage.
    let auto = spots.is_empty();
    let mut state = CycleState::start(bot);
    while !queue.is_empty() {
        if let Some(stop) = poll_stop(bot, &state, &spec) {
            return exit_code(stop);
        }
        let next = queue.iter().position(|queued| !tally.excludes(queued.key));
        let Some(next) = next else {
            bot.log("all remaining spots excluded after deaths, stopping");
            break;
        };
        let spot = queue.remove(next);
        bot.log(&format!("=== {} ===", spot.key));
        bot.intend(&format!("melee at {}", spot.key));
        // Remaining kills under the quantity target (0 = unlimited, as
        // the fight loop reads it); the budget poll above caps ticks.
        let remaining = spec.targets.count.map(|want| want.saturating_sub(state.units())).unwrap_or(0);
        let (total, died, banked) = match crate::kit::fight::run_spot(bot, &spot, remaining, spec.budget_max_ticks, no_bank) {
            Err(crate::ScriptError::Usage(detail)) if auto && detail.contains("toll fence") => {
                if !tally.excludes(spot.key) {
                    tally.exclude(spot.key);
                }
                match crate::kit::fight::fallback_spot(bot, &spot, tally.excluded()) {
                    Some(fallback) => {
                        bot.log(&format!("{} across the toll fence, falling back to {}", spot.key, fallback.key));
                        queue.insert(0, fallback);
                        continue;
                    }
                    None => return Err(crate::ScriptError::Usage(detail)),
                }
            }
            other => other?,
        };
        state.add_units(total);
        if let Some(stop) = poll_stop(bot, &state, &spec) {
            return exit_code(stop);
        }
        if total > 0 || banked {
            tally.note_progress();
        }
        bot.log(&format!("ok {} killed {total}", spot.key));
        if !died {
            continue;
        }
        tally.note_death(spot.key);
        if tally.fatal() {
            return Err(crate::kit::survival::danger_error(spot.key, tally.deaths()));
        }
        if queue.iter().any(|queued| !tally.excludes(queued.key)) {
            bot.log(&format!("{} killed us, re-picking without it", spot.key));
            continue;
        }
        match crate::kit::fight::fallback_spot(bot, &spot, tally.excluded()) {
            Some(fallback) => {
                bot.log(&format!("{} killed us, falling back to {}", spot.key, fallback.key));
                queue.insert(0, fallback);
            }
            None => {
                if !no_bank {
                    crate::kit::gear::prepare_at_bank(bot)?;
                }
                bot.log(&format!("{} killed us, nothing safer on foot: banked and stopping", spot.key));
                break;
            }
        }
    }
    Ok(0)
}
