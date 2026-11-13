//! Magic style: F2P combat spell picks (strikes, bolts, blasts, curses,
//! bind, crumble undead) and the same hunt loop as `melee` (spot queue,
//! death tally, re-pick all live in `kit::fight`, read-only). The kill loop
//! casts (`op_npc_t`) through the shared fight loop (`combat_style`
//! magic, PRD3 lot 6); historical note: the loop used to click attack, a kit
//! change (kit/ frozen this wave), so the picks below are advisory (logged)
//! until then. Every pick is members=false (checked against the dumps).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{exit_code, poll_stop, CycleState};
use crate::kit::survival::check_events;
use crate::ScriptResult;

/// Magic stat id (server order: attack 0 .. magic 6).
pub const MAGIC_STAT: u8 = 6;

/// Damage spells, (debugname, level, max hit), weakest first. F2P only.
pub const DAMAGE_SPELLS: [(&str, i64, i64); 12] = [
    ("wind_strike", 1, 2), ("water_strike", 5, 4), ("earth_strike", 9, 6), ("fire_strike", 13, 8),
    ("wind_bolt", 17, 9), ("water_bolt", 23, 10), ("earth_bolt", 29, 11), ("fire_bolt", 35, 12),
    ("wind_blast", 41, 13), ("water_blast", 47, 14), ("earth_blast", 53, 15), ("fire_blast", 59, 16),
];

/// Weakening curses, (debugname, level), weakest first. F2P only.
pub const CURSES: [(&str, i64); 3] = [("confuse", 3), ("weaken", 11), ("curse", 19)];

/// Bind stops the foe (level 20); crumble undead hits the undead (39).
pub const BIND_LEVEL: i64 = 20;
pub const CRUMBLE_LEVEL: i64 = 39;

/// Current magic level (1 before the stats stream).
pub fn magic_level(bot: &Bot) -> i64 {
    bot.world.stats.get(&MAGIC_STAT).map(|(level, _)| i64::from(*level)).unwrap_or(1)
}

/// Strongest damage spell unlocked at this level (wind_strike at 1).
pub fn best_damage_spell(level: i64) -> &'static str {
    let mut pick = DAMAGE_SPELLS[0].0;
    for (spell, need, _) in DAMAGE_SPELLS {
        if level >= need {
            pick = spell;
        }
    }
    pick
}

/// Max hit of a damage spell, or None for utility spells.
pub fn spell_maxhit(spell: &str) -> Option<i64> {
    DAMAGE_SPELLS.iter().find(|(name, _, _)| *name == spell).map(|(_, _, hit)| *hit)
}

/// Strongest curse unlocked, if any.
pub fn best_curse(level: i64) -> Option<&'static str> {
    CURSES.iter().filter(|(_, need)| level >= *need).map(|(spell, _)| *spell).last()
}

/// True when bind (hold) is castable at this level.
pub fn can_bind(level: i64) -> bool {
    level >= BIND_LEVEL
}

/// True when crumble undead is castable at this level.
pub fn can_crumble(level: i64) -> bool {
    level >= CRUMBLE_LEVEL
}

/// Runes a cast costs. F2P combat set only; unknown spells cost nothing.
pub fn required_runes(spell: &str) -> &'static [(&'static str, i64)] {
    match spell {
        "wind_strike" => &[("mindrune", 1), ("airrune", 1)],
        "water_strike" => &[("mindrune", 1), ("waterrune", 1), ("airrune", 1)],
        "earth_strike" => &[("mindrune", 1), ("earthrune", 2), ("airrune", 1)],
        "fire_strike" => &[("mindrune", 1), ("firerune", 3), ("airrune", 2)],
        "wind_bolt" => &[("chaosrune", 1), ("airrune", 2)],
        "water_bolt" => &[("chaosrune", 1), ("waterrune", 2), ("airrune", 2)],
        "earth_bolt" => &[("chaosrune", 1), ("earthrune", 3), ("airrune", 2)],
        "fire_bolt" => &[("chaosrune", 1), ("firerune", 4), ("airrune", 3)],
        "wind_blast" => &[("deathrune", 1), ("airrune", 3)],
        "water_blast" => &[("deathrune", 1), ("waterrune", 3), ("airrune", 3)],
        "earth_blast" => &[("deathrune", 1), ("earthrune", 4), ("airrune", 3)],
        "fire_blast" => &[("deathrune", 1), ("firerune", 5), ("airrune", 4)],
        "confuse" => &[("bodyrune", 1), ("waterrune", 3), ("earthrune", 2)],
        "weaken" => &[("bodyrune", 1), ("waterrune", 3), ("earthrune", 2)],
        "curse" => &[("bodyrune", 1), ("waterrune", 2), ("earthrune", 3)],
        "bind" => &[("naturerune", 2), ("waterrune", 3), ("earthrune", 3)],
        "crumble_undead" => &[("chaosrune", 1), ("airrune", 2), ("earthrune", 2)],
        _ => &[],
    }
}

/// Strongest damage spell the level unlocks whose runes the pack holds
/// (PRD3 lot 6: the fight loop casts it on the target).
pub fn castable_spell(bot: &Bot) -> Option<&'static str> {
    let level = magic_level(bot);
    DAMAGE_SPELLS.iter().rev().filter(|(_, need, _)| level >= *need).map(|(spell, _, _)| *spell).find(|spell| {
        let runes: Vec<(String, i64)> = required_runes(spell).iter().map(|(name, count)| (name.to_string(), *count)).collect();
        crate::kit::magic::missing_runes(bot, &runes).is_empty()
    })
}

/// Wind-strike runes somewhere in the pack or the bank (last known view:
/// the bank is usually closed here); else blocked on them (the brain
/// files the obtain: market or runecraft).
fn require_combat_gear(bot: &Bot) -> Result<(), crate::ScriptError> {
    let mut missing = Vec::new();
    for rune in ["mindrune", "airrune"] {
        let Ok(id) = crate::kit::pack::obj_id(bot, rune) else { continue };
        let held = crate::kit::pack::pack_total(bot, id) + crate::kit::pack::banked_usable(bot, id);
        if held == 0 {
            missing.push(rune.to_string());
        }
    }
    if missing.is_empty() { Ok(()) } else { Err(crate::ScriptError::Blocked { missing }) }
}

/// Magic farmer: `magic [spot ...] [--kills N] [--ticks N] [--no-bank]`.
pub fn run_magic(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("magic", args)?;
    let spots = spec.resource.clone();
    let no_bank = spec.no_bank;
    crate::kit::toll::arm(bot, spec.allow_paid)?;
    bot.allow_shop = spec.allow_shop;
    crate::kit::survival::wait_pack_streamed(bot)?;
    check_events(bot)?;
    // PRD3 lot 6: the run fights magic for real (gear scored for it,
    // spells cast); missing gear blocks typed for the brain to obtain.
    bot.combat_style = "magic".to_string();
    require_combat_gear(bot)?;
    let picked: Vec<knowledge::combat_spots::CombatSpot> = {
        let named = crate::kit::fight::named_spots(&spots)?;
        if named.is_empty() { vec![knowledge::combat_spots::best_spot(magic_level(bot))] } else { named }
    };
    bot.log(&format!("magic={} spell={} spots={}", magic_level(bot), best_damage_spell(magic_level(bot)), picked.iter().map(|spot| spot.key).collect::<Vec<_>>().join(",")));
    let mut queue = picked;
    let mut tally = crate::kit::survival::DeathTally::fresh();
    let auto = spots.is_empty();
    let mut state = CycleState::begin(bot, &spec);
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
        bot.intend(&format!("magic at {}", spot.key));
        let remaining = spec.targets.count.map(|want| want.saturating_sub(state.units())).unwrap_or(0);
        let (total, died, banked) = match crate::kit::fight::run_spot_with(bot, &spot, &crate::kit::fight::HuntPlan::plain().until(&spec), remaining, spec.budget_max_ticks, no_bank) {
            // An ordered spot behind the toll without `toll`: a typed access
            // gap the brain funds or routes, never a Usage.
            Err(crate::ScriptError::Usage(detail)) if !auto && detail.contains("toll fence") => return Err(crate::kit::survival::ordered_spot_tolled(bot, spot.key, &detail)),
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
        // The brain's spot is an order: never swap it for a "safer" one
        // (a drop farm must stay on the dropper). Bank, report typed.
        if !auto {
            return Err(crate::kit::survival::ordered_spot_lost(bot, spot.key, tally.deaths(), no_bank)?);
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
    use super::{best_curse, best_damage_spell, can_bind, can_crumble, magic_level, required_runes, spell_maxhit, CURSES, DAMAGE_SPELLS};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    #[test]
    fn damage_climbs_with_level() {
        assert_eq!(best_damage_spell(1), "wind_strike");
        assert_eq!(best_damage_spell(4), "wind_strike", "water waits for 5");
        assert_eq!(best_damage_spell(13), "fire_strike");
        assert_eq!(best_damage_spell(17), "wind_bolt");
        assert_eq!(best_damage_spell(35), "fire_bolt");
        assert_eq!(best_damage_spell(59), "fire_blast");
        assert_eq!(best_damage_spell(99), "fire_blast", "ceiling holds");
        assert_eq!(spell_maxhit("fire_blast"), Some(16));
        assert_eq!(spell_maxhit("bind"), None, "utility has no max hit");
    }

    /// Live 2026-11-13 (bdr53jee, leo_king, nina_rhodes: 16 blocks, magic
    /// banned): runes banked, the bank closed (its last contents kept in
    /// `inv_closed`, what the brain reads as held), `bank_items()` empty
    /// (live interface only): "missing mindrune, airrune" at every launch
    /// while the brain's rune obtains closed `already_held`.
    #[test]
    fn banked_runes_pass_the_gear_gate_with_the_bank_closed() {
        use std::collections::HashMap;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert!(matches!(super::require_combat_gear(&bot), Err(crate::ScriptError::Blocked { .. })), "nothing anywhere");
        let main = (bot.tables.com_id)("bank_main:inv").expect("bank component");
        let mind = crate::kit::pack::obj_id(&bot, "mindrune").expect("mind rune");
        let air = crate::kit::pack::obj_id(&bot, "airrune").expect("air rune");
        bot.world.inv_closed.insert(main, HashMap::from([(0u8, (mind, 15u32)), (1, (air, 25))]));
        assert!(super::require_combat_gear(&bot).is_ok(), "banked runes are held, bank closed or not");
    }

    #[test]
    fn curses_bind_and_crumble_gate_on_level() {
        assert_eq!(best_curse(2), None, "too young to curse");
        assert_eq!(best_curse(3), Some("confuse"));
        assert_eq!(best_curse(11), Some("weaken"));
        assert_eq!(best_curse(19), Some("curse"));
        assert!(!can_bind(19), "bind waits for 20");
        assert!(can_bind(20), "bind at 20");
        assert!(!can_crumble(38), "crumble waits for 39");
        assert!(can_crumble(39), "crumble at 39");
        assert_eq!(CURSES.len(), 3, "three F2P curses");
        assert_eq!(DAMAGE_SPELLS.len(), 12, "strikes, bolts, blasts");
    }

    #[test]
    fn rune_costs_match_the_dumps() {
        assert_eq!(required_runes("wind_strike"), &[("mindrune", 1), ("airrune", 1)]);
        assert_eq!(required_runes("fire_blast"), &[("deathrune", 1), ("firerune", 5), ("airrune", 4)]);
        assert!(required_runes("bind").contains(&("naturerune", 2)));
        assert!(required_runes("crumble_undead").contains(&("chaosrune", 1)));
        assert!(required_runes("nope").is_empty(), "unknown spells cost nothing");
    }

    #[test]
    fn every_pick_is_free_to_play() {
        for (spell, _, _) in DAMAGE_SPELLS {
            let rows = lifescape_gamedata::spells::by_name(spell);
            assert!(!rows.is_empty(), "{spell} must exist in the dumps");
            assert!(rows.iter().all(|row| !row.members), "{spell} must be F2P");
        }
        for (spell, _) in CURSES {
            assert!(lifescape_gamedata::spells::by_name(spell).iter().all(|row| !row.members), "{spell} must be F2P");
        }
        for spell in ["bind", "crumble_undead"] {
            assert!(lifescape_gamedata::spells::by_name(spell).iter().all(|row| !row.members), "{spell} must be F2P");
        }
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(6, (35, 0));
        assert_eq!(magic_level(&bot), 35);
    }
}
