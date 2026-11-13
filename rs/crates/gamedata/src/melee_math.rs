//! Melee exchange model, server formulas only (`skill_combat` scripts,
//! rev 274): a fighter's levels and worn bonuses against one NPC's
//! combat block give the kill time, the combat xp per tick and the
//! damage taken. Pure: no bot, no file beyond the static tables.
//!
//! Server truth (`content/scripts/skill_combat`):
//! - `combat_stat(eff, bonus) = eff * (bonus + 64)` (attack and defence
//!   rolls); `combat_maxhit(stat) = (stat + 320) / 640`.
//! - player effective level = level + 8 + style bonus (controlled: +1);
//!   NPC effective level = level + 9 (`npc_combat_melee.rs2`).
//! - a hit lands when `randominc(attack) > randominc(defence)`; damage is
//!   `randominc(maxhit)`, so its mean is `maxhit / 2`.
//! - player xp: 4 combat xp per damage point (hitpoints xp aside).

#![forbid(unsafe_code)]

use crate::npc_combat::NpcCombat;

/// Ticks between two player swings (unarmed, daggers, swords and
/// scimitars; slower weapons only scale every spot alike).
pub const PLAYER_ATTACK_TICKS: f64 = 4.0;
/// Ticks per kill spent outside the exchange: finding a free foe,
/// walking to it, looting, burying the bones, respawn waits. Estimate:
/// it is what makes a 35-hp giant worth more than an 8-hp cow per tick.
pub const KILL_OVERHEAD_TICKS: f64 = 60.0;
/// Natural regeneration: one hitpoint per 100 ticks.
pub const REGEN_TICKS_PER_HP: f64 = 100.0;
/// Combat xp per damage point dealt (the trained melee skill).
pub const XP_PER_DAMAGE: f64 = 4.0;
/// Style bonus of the controlled stance (every melee level +1).
const STYLE_BONUS: i64 = 1;

/// A melee fighter: base levels, max hitpoints, combat level and the
/// worn bonuses (`[stab, slash, crush]` triples).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fighter {
    pub attack: i64,
    pub strength: i64,
    pub defence: i64,
    pub hitpoints: i64,
    pub combat: i64,
    pub attack_bonus: [i64; 3],
    pub strength_bonus: i64,
    pub defence_bonus: [i64; 3],
}

impl Fighter {
    /// No gear: every bonus 0.
    pub fn naked(attack: i64, strength: i64, defence: i64, hitpoints: i64, combat: i64) -> Fighter {
        Fighter { attack, strength, defence, hitpoints, combat, attack_bonus: [0; 3], strength_bonus: 0, defence_bonus: [0; 3] }
    }

    /// The same fighter wearing `gear` (bonuses summed, like the
    /// server's `equip_get_bonuses`).
    pub fn wearing(self, gear: &[crate::equipment::Wearable]) -> Fighter {
        let mut out = Fighter { attack_bonus: [0; 3], strength_bonus: 0, defence_bonus: [0; 3], ..self };
        let lookup = |pairs: &[(String, i64)], key: &str| pairs.iter().find(|(name, _)| name == key).map(|(_, value)| *value).unwrap_or(0);
        for piece in gear {
            for (index, key) in ["stab", "slash", "crush"].iter().enumerate() {
                out.attack_bonus[index] += lookup(&piece.attack, key);
                out.defence_bonus[index] += lookup(&piece.defence, key);
            }
            out.strength_bonus += piece.strength;
        }
        out
    }
}

/// One exchange between a fighter and an NPC, per kill and per tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Ticks the exchange lasts (NPC hitpoints / our damage per tick).
    pub kill_ticks: f64,
    /// Exchange plus the per-kill overhead.
    pub cycle_ticks: f64,
    /// Combat xp per tick over the whole cycle.
    pub xp_per_tick: f64,
    /// Highest single hit the NPC lands on us.
    pub npc_max_hit: i64,
    /// Expected damage we take per kill.
    pub damage_per_kill: f64,
    /// Expected hitpoints lost per tick over the cycle, net of
    /// regeneration (negative: the fighter heals faster than it bleeds).
    pub loss_per_tick: f64,
}

/// `randominc(attack) > randominc(defence)` probability (both uniform
/// on `0..=roll`).
pub fn hit_chance(attack_roll: i64, defence_roll: i64) -> f64 {
    let (attack, defence) = (attack_roll.max(0) as f64, defence_roll.max(0) as f64);
    if attack > defence {
        1.0 - (defence + 2.0) / (2.0 * (attack + 1.0))
    } else {
        attack / (2.0 * (defence + 1.0))
    }
}

/// `combat_maxhit(eff * (bonus + 64))`.
pub fn max_hit(effective_strength: i64, strength_bonus: i64) -> i64 {
    (effective_strength * (strength_bonus + 64) + 320) / 640
}

/// Our damage per tick against `npc`, swinging the attack type our
/// bonuses favour most against its defences. `self_heal_per_tick` is
/// what the NPC heals back (monks), subtracted from our output.
fn damage_per_tick(fighter: &Fighter, npc: &NpcCombat, self_heal_per_tick: f64) -> f64 {
    let effective_attack = fighter.attack + 8 + STYLE_BONUS;
    let npc_defence = npc.defence + 9;
    let best_chance = (0..3)
        .map(|index| hit_chance(effective_attack * (fighter.attack_bonus[index] + 64), npc_defence * (npc.defence_bonus[index] + 64)))
        .fold(0.0, f64::max);
    let ours = max_hit(fighter.strength + 8 + STYLE_BONUS, fighter.strength_bonus);
    best_chance * ours as f64 / 2.0 / PLAYER_ATTACK_TICKS - self_heal_per_tick
}

/// Expected damage the NPC deals per tick, and its max hit (melee
/// NPCs only: a ranged or magic attacker is not modelled).
fn npc_damage(fighter: &Fighter, npc: &NpcCombat) -> Option<(f64, i64)> {
    let index = npc.damage_type.melee_index()?;
    let attack_roll = (npc.attack + 9) * (npc.attack_bonus + 64);
    let defence_roll = (fighter.defence + 8 + STYLE_BONUS) * (fighter.defence_bonus[index] + 64);
    let npc_max = max_hit(npc.strength + 9, npc.strength_bonus);
    let per_tick = hit_chance(attack_roll, defence_roll) * npc_max as f64 / 2.0 / npc.attack_rate as f64;
    Some((per_tick, npc_max))
}

/// The exchange against one NPC; `None` when we cannot hurt it or it
/// does not fight in melee.
pub fn estimate(fighter: &Fighter, npc: &NpcCombat, self_heal_per_tick: f64) -> Option<Estimate> {
    let ours = damage_per_tick(fighter, npc, self_heal_per_tick);
    if ours <= 0.0 {
        return None;
    }
    let (theirs, npc_max_hit) = npc_damage(fighter, npc)?;
    let hitpoints = npc.hitpoints.max(1) as f64;
    let kill_ticks = hitpoints / ours;
    let cycle_ticks = kill_ticks + KILL_OVERHEAD_TICKS;
    let damage_per_kill = theirs * kill_ticks;
    Some(Estimate {
        kill_ticks,
        cycle_ticks,
        xp_per_tick: XP_PER_DAMAGE * hitpoints / cycle_ticks,
        npc_max_hit,
        damage_per_kill,
        loss_per_tick: damage_per_kill / cycle_ticks - 1.0 / REGEN_TICKS_PER_HP,
    })
}

#[cfg(test)]
mod tests {
    use super::{estimate, hit_chance, max_hit, Fighter};
    use crate::npc_combat::by_name;

    /// `randominc` rolls: equal rolls land a bit under half, a far
    /// stronger roll almost always.
    #[test]
    fn hit_chance_follows_the_randominc_rolls() {
        assert!((hit_chance(100, 100) - 100.0 / 202.0).abs() < 1e-9);
        assert!(hit_chance(10_000, 10) > 0.99);
        assert!(hit_chance(10, 10_000) < 0.001);
        assert_eq!(hit_chance(0, 50), 0.0, "no roll, no hit");
    }

    /// Guard max hit from its config: (18 + 9) * (5 + 64) = 1863 -> 3.
    #[test]
    fn max_hits_match_the_server_formula() {
        assert_eq!(max_hit(27, 5), 3, "guard");
        assert_eq!(max_hit(31, 16), 4, "hill giant");
        assert_eq!(max_hit(10, -15), 1, "cow");
    }

    /// The point of the ladder: past the first levels a guard pays more
    /// xp per tick than a cow (the overhead swamps an 8-hp kill), and
    /// costs more hitpoints.
    #[test]
    fn bigger_foes_pay_more_per_tick_and_bite_harder() {
        let fighter = Fighter { attack_bonus: [10, 15, 0], strength_bonus: 14, defence_bonus: [30, 30, 30], ..Fighter::naked(40, 40, 40, 40, 50) };
        let cow = estimate(&fighter, &by_name("cow").expect("cow"), 0.0).expect("cow estimate");
        let guard = estimate(&fighter, &by_name("guard1").expect("guard"), 0.0).expect("guard estimate");
        assert!(guard.xp_per_tick > cow.xp_per_tick * 1.5, "guard {guard:?} vs cow {cow:?}");
        assert!(guard.damage_per_kill > cow.damage_per_kill);
        assert!(cow.loss_per_tick < 0.0, "regeneration outpaces cows");
        assert_eq!(guard.npc_max_hit, 3);
    }
}
