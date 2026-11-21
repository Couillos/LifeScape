//! Gates: whether the bot passes, right now, every access gate gamedata
//! records on a tile (`access::gates_at`, server truth rev 274): a guild
//! door checks the live level and the WORN uniform (`cooking_guild.rs2`,
//! `crafting_guild.rs2`), a key door the carried key, the Champions'
//! Guild door the quest points (varp `qp`). A work site behind a gate the
//! bot does not pass is never picked: the walk-cost ranking alone prices
//! a blocked station at its closest reachable tile, which can be a floor
//! elsewhere (live 2026-11-21, the Cooks' Guild range picked by cooks
//! with no chef's hat worn: 20 `DoorDidNotOpen` in 6 h).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_gamedata::access::Gate;
use lifescape_sdk::Bot;

use crate::kit::pack::{is_worn, obj_id, pack_total};

/// True when the bot passes every gate on (x, z, level); an open tile
/// (no gate) is true.
pub fn tile_open(bot: &Bot, x: i32, z: i32, level: i32) -> bool {
    knowledge::access::gates_at(i64::from(x), i64::from(z), i64::from(level)).iter().all(|gate| gate_open(bot, gate))
}

/// True when the bot passes this gate with its live state. A gate no
/// bot passes today (a quest room, a sealed display, an unmodelled
/// underground, a boat without nav edges) is false; the Karamja boat is
/// a fare the nav pays, true.
pub fn gate_open(bot: &Bot, gate: &Gate) -> bool {
    match gate {
        Gate::Guild { skill, level, uniform, .. } => {
            let live = crate::quests::common::skill_index(skill).map_or(0, |stat| crate::kit::survival::learn_level(bot, stat, 0));
            live >= *level && uniform.map_or(true, |name| obj_id(bot, name).is_ok_and(|obj| is_worn(bot, obj)))
        }
        Gate::Key { item, .. } => obj_id(bot, item).is_ok_and(|obj| pack_total(bot, obj) > 0),
        Gate::QuestPoints { points, .. } => {
            let varp = knowledge::varps::id_by_name("qp").and_then(|id| u16::try_from(id).ok());
            varp.is_some_and(|varp| i64::from(bot.world.varps.get(&varp).copied().unwrap_or(0)) >= *points)
        }
        Gate::Boat { .. } => !knowledge::access::impassable(gate),
        Gate::Quest { .. } | Gate::Sealed { .. } | Gate::Underground => false,
    }
}

#[cfg(test)]
mod tests {
    use super::tile_open;
    use crate::kit::gear::WORN_COM_NAME;
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    fn wear(bot: &mut Bot, name: &str) {
        let obj = obj_id(bot, name).expect("in the dumps");
        let worn_com = (bot.tables.com_id)(WORN_COM_NAME).expect("worn com in the dumps");
        bot.world.inv.insert(worn_com, [(0u8, (obj, 1u32))].into_iter().collect());
    }

    /// `cooking_guild.rs2`: cooking 32 AND the chef's hat worn open the
    /// guild range (3143,3452,1); either alone does not. The street
    /// outside the door is open to all.
    #[test]
    fn the_cooks_guild_needs_level_and_worn_hat() {
        let mut bot = offline_bot();
        assert!(tile_open(&bot, 3143, 3443, 0), "outside the door");
        bot.world.stats.insert(7, (52, 130_000));
        assert!(!tile_open(&bot, 3143, 3452, 1), "cooking 52, no hat worn");
        wear(&mut bot, "chefs_hat");
        assert!(tile_open(&bot, 3143, 3452, 1), "cooking 52, hat worn");
        bot.world.stats.insert(7, (31, 13_000));
        assert!(!tile_open(&bot, 3143, 3452, 1), "hat worn, cooking 31");
    }

    /// `crafting_guild.rs2`: crafting 40 and the brown apron worn; the
    /// Champions' Guild reads the quest-point varp; a quest room never
    /// opens.
    #[test]
    fn other_gates_read_their_own_requirement() {
        let mut bot = offline_bot();
        bot.world.stats.insert(12, (45, 61_512));
        assert!(!tile_open(&bot, 2936, 3290, 0), "crafting 45, no apron");
        wear(&mut bot, "brown_apron");
        assert!(tile_open(&bot, 2936, 3290, 0), "crafting 45, apron worn");
        assert!(!tile_open(&bot, 3191, 3355, 0), "no quest points");
        let qp = lifescape_gamedata::varps::id_by_name("qp").expect("qp varp") as u16;
        bot.world.varps.insert(qp, 32);
        assert!(tile_open(&bot, 3191, 3355, 0), "32 quest points");
        assert!(!tile_open(&bot, 3111, 3367, 0), "the Draynor Manor closet");
    }
}
