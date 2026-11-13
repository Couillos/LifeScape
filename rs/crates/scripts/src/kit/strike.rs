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
/// worn. Magic casts the strongest damage spell the pack funds; no runes
/// left blocks typed so the brain files the runes as an obtain. False
/// when an empty quiver wants a bank trip (arrows banked) instead.
pub(crate) fn attack_npc(bot: &mut Bot, slot: u16, no_bank: bool) -> Result<bool, ScriptError> {
    if bot.combat_style == "magic" {
        let Some(spell) = crate::activities::magic_style::castable_spell(bot) else {
            bot.log("out of runes for any damage spell");
            return Err(ScriptError::Blocked { missing: vec!["mindrune".to_string(), "airrune".to_string()] });
        };
        let com = crate::kit::magic::spell_com(bot, spell)?;
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

#[cfg(test)]
mod tests {
    use super::{foe_refusal, FoeRefusal};

    /// Server refusals split in two: an unreachable foe is worth one
    /// door-aware step (it may burn a door and prove it uncrossable), a
    /// foe already taken by someone else is skipped on the spot — live
    /// (bb4v5d9m 17:52:38) the step toward a taken cow paced ~20 walks.
    #[test]
    fn taken_foe_is_skipped_without_a_step() {
        assert_eq!(foe_refusal("Someone else is fighting that."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("I'm already under attack."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("It's not after you."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("I can't reach that!"), Some(FoeRefusal::Unreachable));
        assert_eq!(foe_refusal("You bury the bones."), None);
    }
}
