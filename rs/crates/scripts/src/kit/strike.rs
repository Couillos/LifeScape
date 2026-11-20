//! Strike: one attack click per run style (melee/ranged Attack, magic
//! cast) and the server's refusals of it, by what the kill loop does
//! about them. Split out of `kit::fight` (kill loop + hunt) to keep that
//! brick focused; ammunition lives in `kit::ammo`.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::ammo::{ensure_ranged_ammo, AmmoFix};
use crate::ScriptError;

/// Server refusal of an attack click (player_combat.rs2), by what the
/// bot should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoeRefusal {
    /// "I can't reach that": one door-aware step may burn the door and
    /// prove it uncrossable (excluded for the whole fight).
    Unreachable,
    /// Someone else holds the foe (or holds us): skip it, never step
    /// toward it.
    Taken,
}

pub(crate) fn foe_refusal(line: &str) -> Option<FoeRefusal> {
    let lowered = line.to_lowercase();
    if lowered.contains("can't reach that") {
        Some(FoeRefusal::Unreachable)
    } else if lowered.contains("already under attack")
        || lowered.contains("someone else is fighting")
        || lowered.contains("not after you")
    {
        Some(FoeRefusal::Taken)
    } else {
        None
    }
}

/// One attack on `slot` for the run's combat style (PRD3 lot 6). Melee
/// and ranged click Attack: the server shoots when a bow and arrows are
/// worn. Magic casts the ordered spell (`bot.combat_spell`), else the
/// strongest damage spell the pack funds; no runes
/// left blocks typed so the brain files the runes as an obtain. False
/// when an empty quiver wants a bank trip (arrows banked) instead.
pub(crate) fn attack_npc(bot: &mut Bot, slot: u16, no_bank: bool) -> Result<bool, ScriptError> {
    if bot.combat_style == "magic" {
        // The planner's spell (`--spell`) and nothing else: its runes run
        // short in the pack, the known bank refills them (a bank trip,
        // like an empty quiver); short there too (or `--no-bank`), the run
        // blocks on exactly those (worn staff element excluded).
        let spell = match bot.combat_spell.clone() {
            Some(ordered) => {
                let runes = crate::activities::magic_style::spell_runes(&ordered);
                let missing = crate::kit::magic::spell_shortfall(bot, &runes);
                if !missing.is_empty() {
                    let unbanked = crate::kit::magic::bank_shortfall(bot, &runes);
                    if !no_bank && unbanked.is_empty() {
                        bot.log(&format!("out of runes for {ordered} in the pack ({}), banked: bank trip", missing.join(", ")));
                        return Ok(false);
                    }
                    let missing = if no_bank { missing } else { unbanked };
                    bot.log(&format!("out of runes for {ordered}: {}", missing.join(", ")));
                    return Err(ScriptError::Blocked { missing });
                }
                ordered
            }
            None => match castable_spell(bot) {
                Some(best) => best.to_string(),
                None if !no_bank && crate::kit::magic::bank_shortfall(bot, &crate::activities::magic_style::spell_runes("wind_strike")).is_empty() => {
                    bot.log("out of runes in the pack, wind strike's banked: bank trip");
                    return Ok(false);
                }
                None => {
                    bot.log("out of runes for any damage spell");
                    return Err(ScriptError::Blocked { missing: vec!["mindrune".to_string(), "airrune".to_string()] });
                }
            },
        };
        let com = crate::kit::magic::spell_com(bot, &spell)?;
        bot.op_npc_t(slot, com)?;
        return Ok(true);
    }
    // Empty quiver: pack, then the arrows on the ground, then the bank
    // (trip) or blocked typed for the brain to obtain more.
    if bot.combat_style == "ranged" && ensure_ranged_ammo(bot, no_bank)? == AmmoFix::BankTrip {
        return Ok(false);
    }
    bot.op_npc(2, slot)?;
    Ok(true)
}

/// Ticks between re-attacks while engaged: a manual cast never repeats
/// (one spell per click, 5-tick cast speed); melee and ranged keep
/// fighting on their own and only need a nudge.
pub(crate) fn reattack_ticks(bot: &Bot) -> u64 {
    if bot.combat_style == "magic" { 5 } else { 25 }
}

/// Re-attack while engaged (the woodcut rule for combat): a manual cast
/// is one spell per click, so magic re-casts on its cadence; melee and
/// ranged fight on their own (`player_combat.rs2` re-queues the swing),
/// so they are nudged only once the fight went quiet — no combat xp and
/// no step for a whole window — never every 25 ticks while xp flows.
/// Pure so the cadence is locked by tests.
pub(crate) fn reattack_due(magic: bool, window: u64, since_attack: u64, since_progress: u64) -> bool {
    if magic {
        return since_attack >= window;
    }
    since_attack >= window && since_progress >= window
}

/// Strongest damage spell the magic level unlocks whose runes the pack
/// (or a worn staff) funds (PRD3 lot 6: the fight loop casts it on the
/// target).
pub fn castable_spell(bot: &Bot) -> Option<&'static str> {
    use crate::activities::magic_style::{magic_level, spell_runes, DAMAGE_SPELLS};
    let level = magic_level(bot);
    DAMAGE_SPELLS.iter().rev().filter(|(_, need, _)| level >= *need).map(|(spell, _, _)| *spell).find(|spell| {
        crate::kit::magic::spell_shortfall(bot, &spell_runes(spell)).is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::{foe_refusal, FoeRefusal};

    /// Server refusals split in two: an unreachable foe is worth one
    /// door-aware step (it may burn a door and prove it uncrossable), a
    /// foe already taken by someone else is skipped on the spot — live
    /// (bb4v5d9m 17:52:38) the step toward a taken cow paced ~20 walks.
    /// Melee/ranged never re-click while xp flows; a quiet fight gets one
    /// nudge per window; magic casts on its 5-tick cadence.
    #[test]
    fn engaged_melee_waits_while_the_fight_runs() {
        use super::reattack_due;
        assert!(!reattack_due(false, 25, 25, 3), "xp 3 ticks ago: the swing loop runs, no click");
        assert!(!reattack_due(false, 25, 400, 24), "long fight, steady xp: still no click");
        assert!(reattack_due(false, 25, 25, 25), "quiet for a window: one nudge");
        assert!(!reattack_due(false, 25, 10, 60), "nudged 10 ticks ago: wait the window");
        assert!(reattack_due(true, 5, 5, 0), "magic: one cast per click on the cast cadence");
    }

    #[test]
    fn taken_foe_is_skipped_without_a_step() {
        assert_eq!(foe_refusal("Someone else is fighting that."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("I'm already under attack."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("It's not after you."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("I can't reach that!"), Some(FoeRefusal::Unreachable));
        assert_eq!(foe_refusal("You bury the bones."), None);
    }

    /// Magic run out of the ordered spell's runes in the pack while the
    /// known bank holds them: a bank trip (`Ok(false)`, the hunt re-banks
    /// through `prepare_at_bank`), never `blocked` on banked runes. With
    /// `--no-bank`, or nothing banked, blocked by exact names.
    #[test]
    fn banked_runes_send_a_magic_run_to_the_bank() {
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("mindrune", 3)], &[("firerune", 30), ("airrune", 30)]);
        bot.combat_style = "magic".to_string();
        bot.combat_spell = Some("fire_strike".to_string());
        assert!(matches!(super::attack_npc(&mut bot, 1, false), Ok(false)), "bank trip for the banked runes");
        match super::attack_npc(&mut bot, 1, true) {
            Err(crate::ScriptError::Blocked { mut missing }) => {
                missing.sort();
                assert_eq!(missing, vec!["airrune".to_string(), "firerune".to_string()]);
            }
            other => panic!("no-bank run blocks, got {other:?}"),
        }
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("mindrune", 3)], &[("firerune", 30)]);
        bot.combat_style = "magic".to_string();
        bot.combat_spell = Some("fire_strike".to_string());
        match super::attack_npc(&mut bot, 1, false) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["airrune".to_string()], "only the air is nowhere"),
            other => panic!("air nowhere blocks, got {other:?}"),
        }
    }
}
