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
//!
//! Ranged and magic fighters (user 2026-11-20: every style climbs the
//! same camps) roll like `player_combat_stat.rs2` and the NPC defence
//! rolls of `npc_combat.rs2`: ranged attack = (level + 8 + 3, accurate)
//! x (range bonus + 64) against (defence + 9) x (`rangedefence` + 64),
//! max hit `combat_maxhit(effective x (arrow strength + 64))`, a shot
//! every 4 ticks, 4 xp per damage; magic attack = (level + 8 + 1) x
//! (magic bonus + 64) against (NPC magic + 9) x (`magicdefence` + 64),
//! the spell's max hit, a cast every 5 ticks, the spell's base xp plus
//! 2 per damage. The NPC answers in melee whatever the style.

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
/// Ticks between two shots of a shortbow (default `attackrate`).
pub const RANGED_ATTACK_TICKS: f64 = 4.0;
/// Ticks between two combat casts.
pub const MAGIC_ATTACK_TICKS: f64 = 5.0;
/// Ranged style bonus of the accurate style (`combat_get_damagestyle_
/// bonuses`: `^style_ranged_accurate` = +3 ranged).
pub const RANGED_ACCURATE_BONUS: i64 = 3;
/// Magic's fixed style bonus (`player_combat_stat.rs2`: always +1).
pub const MAGIC_STYLE_BONUS: i64 = 1;
/// Ranged strength of a bronze arrow (`rangebonus`), the ammo assumed
/// when no arrow is worn.
pub const BRONZE_ARROW_STRENGTH: i64 = 7;
/// Ranged xp per damage point (like melee).
pub const RANGED_XP_PER_DAMAGE: f64 = 4.0;
/// Magic xp per damage point, on top of the spell's base xp.
pub const MAGIC_XP_PER_DAMAGE: f64 = 2.0;

/// How the fighter deals its damage. The NPC answers in melee whatever
/// the style (no safe spot: the hunt walks into the camp).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Offense {
    /// Attack and strength with the worn melee bonuses.
    Melee,
    /// Ranged level, worn ranged attack bonus, arrow ranged strength.
    Ranged { level: i64, attack_bonus: i64, arrow_strength: i64 },
    /// Magic level, worn magic attack bonus, the cast spell's max hit
    /// and base xp per cast.
    Magic { level: i64, attack_bonus: i64, max_hit: i64, cast_xp: f64 },
}

/// A fighter: base levels, max hitpoints, combat level, the worn melee
/// bonuses (`[stab, slash, crush]` triples) and its offense.
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
    pub offense: Offense,
}

impl Fighter {
    /// No gear: every bonus 0, melee.
    pub fn naked(attack: i64, strength: i64, defence: i64, hitpoints: i64, combat: i64) -> Fighter {
        Fighter { attack, strength, defence, hitpoints, combat, attack_bonus: [0; 3], strength_bonus: 0, defence_bonus: [0; 3], offense: Offense::Melee }
    }

    /// The same fighter shooting: ranged `level`, worn ranged attack
    /// `attack_bonus`, arrows of `arrow_strength`.
    pub fn shooting(self, level: i64, attack_bonus: i64, arrow_strength: i64) -> Fighter {
        Fighter { offense: Offense::Ranged { level, attack_bonus, arrow_strength }, ..self }
    }

    /// The same fighter casting a spell of `max_hit` and `cast_xp` base
    /// xp: magic `level`, worn magic attack `attack_bonus`.
    pub fn casting(self, level: i64, attack_bonus: i64, max_hit: i64, cast_xp: f64) -> Fighter {
        Fighter { offense: Offense::Magic { level, attack_bonus, max_hit, cast_xp }, ..self }
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

/// Odds a ranged shot lands on `npc` and its max hit, at ranged
/// `level` with `attack_bonus` worn and arrows of `arrow_strength`.
pub fn ranged_roll(npc: &NpcCombat, level: i64, attack_bonus: i64, arrow_strength: i64) -> (f64, i64) {
    let effective = level.max(1) + 8 + RANGED_ACCURATE_BONUS;
    let attack = effective * (attack_bonus + 64);
    let defence = (npc.defence + 9) * (npc.ranged_defence + 64);
    (hit_chance(attack, defence), max_hit(effective, arrow_strength))
}

/// Odds a combat cast lands on `npc` at magic `level` with
/// `attack_bonus` worn (the max hit is the spell's).
pub fn magic_odds(npc: &NpcCombat, level: i64, attack_bonus: i64) -> f64 {
    let effective = level.max(1) + 8 + MAGIC_STYLE_BONUS;
    hit_chance(effective * (attack_bonus + 64), (npc.magic + 9) * (npc.magic_defence + 64))
}

/// Landing odds, max hit and attack interval of the fighter's offense
/// against `npc` (melee: the attack type its bonuses favour most).
fn offense_roll(fighter: &Fighter, npc: &NpcCombat) -> (f64, i64, f64) {
    match fighter.offense {
        Offense::Melee => {
            let effective_attack = fighter.attack + 8 + STYLE_BONUS;
            let npc_defence = npc.defence + 9;
            let best_chance = (0..3)
                .map(|index| hit_chance(effective_attack * (fighter.attack_bonus[index] + 64), npc_defence * (npc.defence_bonus[index] + 64)))
                .fold(0.0, f64::max);
            (best_chance, max_hit(fighter.strength + 8 + STYLE_BONUS, fighter.strength_bonus), PLAYER_ATTACK_TICKS)
        }
        Offense::Ranged { level, attack_bonus, arrow_strength } => {
            let (odds, most) = ranged_roll(npc, level, attack_bonus, arrow_strength);
            (odds, most, RANGED_ATTACK_TICKS)
        }
        Offense::Magic { level, attack_bonus, max_hit, .. } => (magic_odds(npc, level, attack_bonus), max_hit, MAGIC_ATTACK_TICKS),
    }
}

/// Our damage per tick against `npc` (`offense_roll`). `self_heal_per_
/// tick` is what the NPC heals back (monks), subtracted from our output.
fn damage_per_tick(fighter: &Fighter, npc: &NpcCombat, self_heal_per_tick: f64) -> f64 {
    let (odds, most, interval) = offense_roll(fighter, npc);
    odds * most as f64 / 2.0 / interval - self_heal_per_tick
}

/// Xp one kill of an `hitpoints`-hp NPC pays the fighter's trained
/// skill over `kill_ticks` of exchange (magic: the base xp of every
/// cast on top of the damage).
fn kill_xp(fighter: &Fighter, hitpoints: f64, kill_ticks: f64) -> f64 {
    match fighter.offense {
        Offense::Melee => XP_PER_DAMAGE * hitpoints,
        Offense::Ranged { .. } => RANGED_XP_PER_DAMAGE * hitpoints,
        Offense::Magic { cast_xp, .. } => MAGIC_XP_PER_DAMAGE * hitpoints + cast_xp * kill_ticks / MAGIC_ATTACK_TICKS,
    }
}

/// Melee max hit of an NPC by name (`npc_combat_melee.rs2`: effective
/// strength = level + 9); `None` without a melee combat block (magic
/// and ranged attackers are not modelled).
pub fn npc_max_hit(name: &str) -> Option<i64> {
    let npc = crate::npc_combat::by_name(name)?;
    npc.damage_type.melee_index()?;
    Some(max_hit(npc.strength + 9, npc.strength_bonus))
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
        xp_per_tick: kill_xp(fighter, hitpoints, kill_ticks) / cycle_ticks,
        npc_max_hit,
        damage_per_kill,
        loss_per_tick: damage_per_kill / cycle_ticks - 1.0 / REGEN_TICKS_PER_HP,
    })
}

#[cfg(test)]
mod tests {
    use super::{estimate, hit_chance, max_hit, npc_max_hit, Fighter};

    /// Max hits from the server formula: a man hits 1, a hill giant
    /// more than a goblin; an unknown NPC has none.
    #[test]
    fn npc_max_hits_follow_the_formula() {
        let man = npc_max_hit("man").expect("man melee");
        let giant = npc_max_hit("giant").expect("giant melee");
        assert!(man >= 1 && man < giant, "man {man} giant {giant}");
        assert_eq!(npc_max_hit("no_such_npc"), None);
    }
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

    /// Every style rates the same camps (user 2026-11-20): an archer and
    /// a mage hurt a giant with their own levels and gear, never with
    /// their (untrained) melee; the giant answers on their defence alike.
    #[test]
    fn archers_and_mages_rate_with_their_own_levels() {
        let giant = by_name("giant").expect("hill giant");
        let untrained = Fighter::naked(1, 1, 40, 40, 40);
        let melee = estimate(&untrained, &giant, 0.0).expect("weak but rated");
        let archer = estimate(&untrained.shooting(60, 20, 22), &giant, 0.0).expect("archer");
        let mage = estimate(&untrained.casting(60, 10, 12, 13.5), &giant, 0.0).expect("mage");
        assert!(archer.xp_per_tick > melee.xp_per_tick * 3.0, "archer {archer:?} vs fists {melee:?}");
        assert!(mage.xp_per_tick > melee.xp_per_tick * 3.0, "mage {mage:?} vs fists {melee:?}");
        assert_eq!(archer.npc_max_hit, melee.npc_max_hit, "the giant answers in melee whatever the style");
        let bronze = estimate(&untrained.shooting(60, 20, super::BRONZE_ARROW_STRENGTH), &giant, 0.0).expect("bronze");
        assert!(archer.kill_ticks < bronze.kill_ticks, "better arrows kill faster");
    }
}
