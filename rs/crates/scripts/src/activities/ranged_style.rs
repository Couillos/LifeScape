//! Ranged style: F2P bow + arrow picks, arrow recovery, and the same
//! hunt loop as `melee` (spot queue, death tally, re-pick all live in
//! `kit::fight`, read-only). Safe spots need router support (single-level
//! today): debt, not worked around — the hunt walks to the cluster like
//! melee. Gear still scores melee in the kit (`prepare_at_bank`): the bow
//! and arrow picks below are advisory (logged) until the kit learns ranged
//! weights. The kill loop clicks attack, not the bow: true kiting needs a
//! kit change (kit/ frozen this wave).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{exit_code, poll_stop, CycleState};
use crate::kit::survival::check_events;
use crate::ScriptResult;

/// Ranged stat id (server order: attack 0, defence 1, strength 2,
// hitpoints 3, ranged 4, prayer 5, magic 6).
pub const RANGED_STAT: u8 = 4;

/// F2P bows, strongest first (dump `range` bonus: oak 14, plain 8; short
/// before long on ties — same bonus, faster shot).
pub const BOWS: [&str; 4] = ["oak_shortbow", "oak_longbow", "shortbow", "longbow"];

/// Arrow tiers, strongest first (bronze < iron < steel < mithril < adamant
/// < rune, standard metal order).
pub const ARROW_TIERS: [&str; 6] =
    ["rune_arrow", "adamant_arrow", "mithril_arrow", "steel_arrow", "iron_arrow", "bronze_arrow"];

/// Current ranged level (1 before the stats stream).
pub fn ranged_level(bot: &Bot) -> i64 {
    bot.world.stats.get(&RANGED_STAT).map(|(level, _)| i64::from(*level)).unwrap_or(1)
}

/// Strongest bow among the carried names, if any.
pub fn best_bow<'held>(held: &[&'held str]) -> Option<&'held str> {
    BOWS.iter().find(|bow| held.contains(bow)).copied()
}

/// Arrow tier index (0 = strongest) of a name, `_p` poison variants included.
pub fn arrow_tier(name: &str) -> Option<usize> {
    let lowered = name.to_lowercase();
    let base: &str = lowered.strip_suffix("_p").unwrap_or(&lowered);
    ARROW_TIERS.iter().position(|tier| *tier == base)
}

/// True for a shootable arrow (plain or poisoned), never a head or shaft.
pub fn is_arrow(name: &str) -> bool {
    arrow_tier(name).is_some()
}

/// Strongest carried arrow, if any.
pub fn best_arrows<'held>(held: &[&'held str]) -> Option<&'held str> {
    held.iter().filter_map(|name| arrow_tier(name).map(|tier| (tier, *name))).min_by_key(|(tier, _)| *tier).map(|(_, name)| name)
}

/// Auto pick: foodless beginners hug chickens (same shelter as melee), the
/// rest hunt the tier their ranged level earned.
fn shelter_or_best(bot: &Bot) -> knowledge::combat_spots::CombatSpot {
    let broke = ranged_level(bot) <= 2
        && crate::kit::survival::food_count(bot) == 0
        && !crate::kit::survival::bank_stocks_food(bot);
    if broke {
        if let Some(chickens) = knowledge::combat_spots::by_key("chickens") {
            return chickens;
        }
    }
    knowledge::combat_spots::best_spot(ranged_level(bot))
}

/// A bow and arrows somewhere in the pack, worn or banked; else blocked
/// on the cheapest of each (the brain files the obtain).
fn require_combat_gear(bot: &Bot) -> Result<(), crate::ScriptError> {
    // Debug names (`oak_shortbow`): the table speaks display names.
    let owned: Vec<String> = crate::kit::gear::available_gear(bot)
        .into_keys()
        .filter_map(|id| knowledge::obj_by_id(usize::from(id)).and_then(|obj| obj.name).map(str::to_string))
        .collect();
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let mut missing = Vec::new();
    if best_bow(&refs).is_none() {
        missing.push("shortbow".to_string());
    }
    if best_arrows(&refs).is_none() {
        missing.push("bronze_arrow".to_string());
    }
    if missing.is_empty() { Ok(()) } else { Err(crate::ScriptError::Blocked { missing }) }
}

/// Ranged farmer: `ranged [spot ...] [--kills N] [--ticks N] [--no-bank]`.
pub fn run_ranged(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("ranged", args)?;
    let spots = spec.resource.clone();
    let no_bank = spec.no_bank;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    crate::kit::survival::wait_pack_streamed(bot)?;
    check_events(bot)?;
    // PRD3 lot 6: the run fights ranged for real (gear scored for it,
    // bow + arrows worn); missing gear blocks typed for the brain to obtain.
    bot.combat_style = "ranged".to_string();
    require_combat_gear(bot)?;
    let picked: Vec<knowledge::combat_spots::CombatSpot> = {
        let named = crate::kit::fight::named_spots(&spots)?;
        if named.is_empty() { vec![shelter_or_best(bot)] } else { named }
    };
    bot.log(&format!("ranged={} spots={}", ranged_level(bot), picked.iter().map(|spot| spot.key).collect::<Vec<_>>().join(",")));
    let mut queue = picked;
    let mut tally = crate::kit::survival::DeathTally::fresh();
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
        bot.intend(&format!("ranged at {}", spot.key));
        let remaining = spec.targets.count.map(|want| want.saturating_sub(state.units())).unwrap_or(0);
        let (total, died, banked) = match crate::kit::fight::run_spot_with(bot, &spot, &crate::kit::fight::HuntPlan::plain().until(&spec), remaining, spec.budget_max_ticks, no_bank) {
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

#[cfg(test)]
mod tests {
    use super::{arrow_tier, best_arrows, best_bow, is_arrow, ranged_level, ARROW_TIERS, BOWS};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn ranged_bot(ranged: u8) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(4, (ranged, 0));
        bot
    }

    #[test]
    fn bow_prefers_the_stronger_draw() {
        assert_eq!(best_bow(&["shortbow", "longbow"]), Some("shortbow"));
        assert_eq!(best_bow(&["longbow", "oak_shortbow"]), Some("oak_shortbow"), "oak outdraws plain");
        assert_eq!(best_bow(&["bronze_sword"]), None, "no bow held");
        assert_eq!(best_bow(&[]), None, "empty hands");
    }

    #[test]
    fn arrows_rank_by_metal_poison_kept() {
        assert!(is_arrow("bronze_arrow"), "plain bronze shoots");
        assert!(is_arrow("steel_arrow_p"), "poisoned steel shoots");
        assert!(!is_arrow("shortbow"), "the bow is not ammo");
        assert!(!is_arrow("arrow_shaft"), "unheaded shafts never shoot");
        assert!(!is_arrow("bones"), "bones never shoot");
        assert_eq!(best_arrows(&["bronze_arrow", "iron_arrow"]), Some("iron_arrow"), "iron beats bronze");
        assert_eq!(best_arrows(&["bronze_arrow_p", "bronze_arrow"]), Some("bronze_arrow_p"), "tie keeps the first");
        assert_eq!(best_arrows(&["net"]), None, "no arrows held");
    }

    #[test]
    fn arrow_tiers_climb_with_metal() {
        let bronze = arrow_tier("bronze_arrow").expect("bronze shoots");
        let rune = arrow_tier("rune_arrow").expect("rune shoots");
        assert!(rune < bronze, "rune outranks bronze");
        assert_eq!(arrow_tier("steel_arrow_p"), arrow_tier("steel_arrow"), "poison never changes the tier");
        assert_eq!(ARROW_TIERS.len(), 6, "six F2P tiers");
        assert_eq!(BOWS.len(), 4, "four F2P bows");
    }

    #[test]
    fn ranged_level_reads_stat_four() {
        assert_eq!(ranged_level(&ranged_bot(40)), 40);
        assert_eq!(ranged_level(&Bot::new("127.0.0.1", 43594, ".", ".")), 1, "unstreamed stats read 1");
    }
}
