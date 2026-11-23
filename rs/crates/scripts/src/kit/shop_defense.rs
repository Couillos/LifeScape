//! Defence of a shop sale under attack: every hit closes the shop
//! (`damage_self` runs `if_close`), so the bot kills what attacks it
//! (Bandit Camp: `brawling_bandit` and `guard_bandit`, both `hunts_players`, level 22, 27 hp, see
//! content `bandit_camp.npc`), eating by the fight's meal rule, before the
//! shop is reopened. Reuses `kit::meals` and `kit::survival`; the sale
//! only calls `defend`.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::meals::{fight_meal, foe_max_hit, Meal};
use crate::kit::survival::{hitpoints, max_hitpoints};
use crate::ScriptError;

/// Ticks one defence may last before it fails typed.
pub const DEFENCE_TICKS: u64 = 600;
/// Ticks between two Attack clicks on the same foe.
const RECLICK_TICKS: u64 = 5;

/// Npcs fighting us right now: slot and tile of every non-staff npc
/// whose target (FACE_ENTITY) is our player slot.
pub fn attackers(bot: &Bot) -> Vec<(u16, i32, i32)> {
    let Some(me) = bot.world.self_slot else { return Vec::new() };
    bot.world
        .npcs
        .iter()
        .filter(|(_, npc)| lifescape_sdk::engagement::targeted_player(npc) == Some(me) && !crate::kit::foes::is_shop_staff(npc.npc_type))
        .map(|(slot, npc)| (*slot, npc.x, npc.z))
        .collect()
}

/// Pure: the food verdict of a defence turn. Starving (emergency
/// hitpoints, nothing edible) ends the defence typed, never a flight.
pub fn defence_over(meal: Meal) -> bool {
    meal == Meal::Starving
}

/// Kills what attacks us until nobody does, eating as needed. Returns
/// nothing once nobody attacks us. Typed `Failed` when the
/// bot dies, starves under attack, or the defence passes `DEFENCE_TICKS`.
pub fn defend(bot: &mut Bot, names: &[&str]) -> Result<(), ScriptError> {
    let started = bot.world.ticks;
    let max_hit = foe_max_hit(names);
    let mut last_click = 0u64;
    loop {
        let foes = attackers(bot);
        if foes.is_empty() {
            return Ok(());
        }
        if bot.world.ticks.saturating_sub(started) >= DEFENCE_TICKS {
            return Err(ScriptError::Failed(format!("shopsell: {} attackers still alive after {DEFENCE_TICKS} ticks at {:?}", foes.len(), bot.position())));
        }
        if hitpoints(bot) == 0 {
            return Err(ScriptError::Failed(format!("shopsell: died under attack at {:?}", bot.position())));
        }
        if defence_over(fight_meal(bot, max_hit)?) {
            return Err(ScriptError::Failed(format!("shopsell: out of food at hp {}/{} under attack at {:?}", hitpoints(bot), max_hitpoints(bot), bot.position())));
        }
        if bot.world.ticks.saturating_sub(last_click) >= RECLICK_TICKS || last_click == 0 {
            last_click = bot.world.ticks;
            bot.log(&format!("shopsell: {} attacker(s), fighting npc {} at ({},{})", foes.len(), foes[0].0, foes[0].1, foes[0].2));
            bot.op_npc(2, foes[0].0)?;
        }
        bot.pump(2)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_sdk::world::NpcInfo;

    fn npc(npc_type: u16, face_entity: Option<u16>) -> NpcInfo {
        NpcInfo { npc_type, x: 3025, z: 3703, level: 0, name: None, anim: None, face_entity, hit_tick: None, spotanim: None }
    }

    /// User 2026-11-23: the bandits that interrupt the sale are killed,
    /// then the shop reopens. Only npcs targeting us count (a bandit on
    /// another player, or idle, is no attacker; the keeper never is).
    #[test]
    fn only_the_npcs_targeting_us_are_attackers() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.self_slot = Some(12);
        let bandit = (0..8000usize).find(|id| lifescape_gamedata::npc_by_id(*id).and_then(|entry| entry.name).is_some_and(|name| name == "brawling_bandit") && !crate::kit::foes::is_shop_staff(*id as u16)).expect("brawling_bandit in the dump") as u16;
        bot.world.npcs.insert(1, npc(bandit, Some(32768 + 12)));
        bot.world.npcs.insert(2, npc(bandit, Some(32768 + 13)));
        bot.world.npcs.insert(3, npc(bandit, None));
        assert_eq!(attackers(&bot), vec![(1, 3025, 3703)]);
        bot.world.self_slot = None;
        assert!(attackers(&bot).is_empty(), "our slot unknown: nobody is read as attacking us");
        assert!(defence_over(Meal::Starving));
        assert!(!defence_over(Meal::Ate) && !defence_over(Meal::Held));
    }
}
