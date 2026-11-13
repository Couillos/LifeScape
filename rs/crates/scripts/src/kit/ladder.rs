//! Live side of the melee ladder (`gamedata::melee_ladder`): reads the
//! bot (base levels, worn bonuses, food and coins in pack and bank, the
//! brass key, the toll flank), asks the ladder and turns its pick into
//! a spot plus a `HuntPlan`. A foodless bot whose best rung bites gets
//! the typed `Blocked` the brain turns into a food obtain.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use knowledge::melee_ladder::{Gate, Holdings, Pick, Rung, BRASS_KEY, FOOD_ITEM, TOLL_COINS};
use knowledge::melee_math::Fighter;

use crate::kit::gear::WORN_COM_NAME;
use crate::kit::hunt::{food_blocked, HuntPlan};
use crate::ScriptError;

/// Spot keyword that asks the ladder (`melee ladder`).
pub const LADDER_KEY: &str = "ladder";

/// Base level of a stat from its xp (never the boosted or drained
/// current level); `fresh` before the stats stream.
fn base_level(bot: &Bot, stat: u8, fresh: i64) -> i64 {
    bot.world.stats.get(&stat).map(|(_, exp)| i64::from(lifescape_sdk::snapshot::level_for_xp(*exp))).unwrap_or(fresh)
}

/// Wearables the bot has on (worn inventory).
fn worn_gear(bot: &Bot) -> Vec<knowledge::equipment::Wearable> {
    let Some(worn_com) = (bot.tables.com_id)(WORN_COM_NAME) else { return Vec::new() };
    let Some(slots) = bot.world.inv.get(&worn_com) else { return Vec::new() };
    let worn: Vec<u16> = slots.values().map(|(held, _)| *held).collect();
    knowledge::equipment::all_wearables()
        .into_iter()
        .filter(|entry| crate::kit::pack::obj_id(bot, &entry.name).is_ok_and(|obj| worn.contains(&obj)))
        .collect()
}

/// The fighter as the ladder rates it: base levels, max hitpoints,
/// combat level and the worn bonuses.
pub fn fighter(bot: &Bot) -> Fighter {
    let (attack, defence, strength, hitpoints) = (base_level(bot, 0, 1), base_level(bot, 1, 1), base_level(bot, 2, 1), base_level(bot, 3, 10));
    let combat = bot.combat_level().map(i64::from).unwrap_or(3);
    Fighter::naked(attack, strength, defence, hitpoints, combat).wearing(&worn_gear(bot))
}

/// Units and best heal of the edible stacks in `stacks` (obj id, count).
fn food_in(bot: &Bot, stacks: impl IntoIterator<Item = (u16, u32)>) -> (u32, i64) {
    stacks.into_iter().fold((0, 0), |(units, best), (held, count)| {
        let heal = (bot.tables.obj_name)(held).map(|name| knowledge::food::heal(&name)).unwrap_or(0);
        if heal > 0 {
            (units + count, best.max(heal))
        } else {
            (units, best)
        }
    })
}

/// Pure holdings from counted parts (tests and the live read share it).
pub fn holdings_from(pack_food: (u32, i64), bank_food: (u32, i64), coins: u32, brass_key: bool, east_of_toll: bool) -> Holdings {
    Holdings { food_count: pack_food.0 + bank_food.0, best_heal: pack_food.1.max(bank_food.1), coins, brass_key, east_of_toll }
}

/// What the bot owns, pack plus the last bank view.
pub fn holdings(bot: &mut Bot) -> Holdings {
    let pack: Vec<(u16, u32)> = bot.backpack().1.into_iter().map(|(_, stack)| stack).collect();
    let bank: Vec<(u16, u32)> = bot.bank_items().into_iter().collect();
    let count_of = |stacks: &[(u16, u32)], wanted: Option<u16>| -> u32 {
        wanted.map(|obj| stacks.iter().filter(|(held, _)| *held == obj).map(|(_, count)| *count).sum()).unwrap_or(0)
    };
    let coins_id = crate::kit::pack::obj_id(bot, "coins").ok();
    let key_id = crate::kit::pack::obj_id(bot, BRASS_KEY).ok();
    let coins = count_of(&pack, coins_id) + count_of(&bank, coins_id);
    let brass_key = count_of(&pack, key_id) + count_of(&bank, key_id) > 0;
    let east_of_toll = crate::kit::bank::toll_gate_x(bot).is_some_and(|line| bot.world.x >= line);
    holdings_from(food_in(bot, pack.iter().copied()), food_in(bot, bank.iter().copied()), coins, brass_key, east_of_toll)
}

/// Items a rung's gate needs on every trip (name, units).
pub fn gate_items(rung: &Rung, holdings: &Holdings) -> Vec<(String, u32)> {
    match rung.gate {
        Gate::Open => Vec::new(),
        Gate::Toll if holdings.east_of_toll => Vec::new(),
        Gate::Toll => vec![("coins".to_string(), TOLL_COINS)],
        Gate::BrassKey => vec![(BRASS_KEY.to_string(), 1)],
    }
}

/// The plan of one hunt pick (pure: tests drive it without a bot).
pub fn plan_of(rung: &Rung, clusters: &[(i64, i64, i64, usize)], food: Option<u32>, holdings: &Holdings) -> HuntPlan {
    HuntPlan {
        clusters: Some(clusters.iter().map(|(x, z, _, spawns)| (*x as i32, *z as i32, *spawns)).collect()),
        food_target: food.unwrap_or(crate::kit::survival::FOOD_TARGET as u32),
        food_required: food.is_some(),
        keep: gate_items(rung, holdings),
        stop_level: None,
    }
}

/// Ask the ladder (rungs in `excluded` left out: killers of this run).
/// Hunt: the spot and its plan (the toll opened when the rung needs it).
/// NeedFood: typed `Blocked` on `FOOD_ITEM`. Nothing workable: Failed.
pub fn pick(bot: &mut Bot, excluded: &[&str]) -> Result<(knowledge::combat_spots::CombatSpot, HuntPlan), ScriptError> {
    let melee = fighter(bot);
    let owned = holdings(bot);
    let seed = knowledge::melee_ladder::seed_of(&bot.username);
    let choice = knowledge::melee_ladder::choose_excluding(&melee, &owned, seed, excluded);
    bot.log(&format!(
        "ladder: atk={} str={} def={} hp={} cb={} bonus atk={:?} str={} def={:?} food={}x{} coins={} key={} -> {choice:?}",
        melee.attack,
        melee.strength,
        melee.defence,
        melee.hitpoints,
        melee.combat,
        melee.attack_bonus,
        melee.strength_bonus,
        melee.defence_bonus,
        owned.food_count,
        owned.best_heal,
        owned.coins,
        owned.brass_key,
    ));
    match choice {
        Some(Pick::Hunt { rung, clusters, food }) => {
            let spot = knowledge::combat_spots::by_key(rung.key)
                .ok_or_else(|| ScriptError::Usage(format!("ladder rung {} is no combat spot", rung.key)))?;
            if rung.gate == Gate::Toll && !owned.east_of_toll {
                bot.allow_paid = true;
                bot.log(&format!("ladder: {} across the toll, paying the gate ({TOLL_COINS} coins kept)", rung.key));
            }
            Ok((spot, plan_of(&rung, &clusters, food, &owned)))
        }
        Some(Pick::NeedFood { rung, count }) => {
            bot.log(&format!("ladder: {} needs food ({count} {FOOD_ITEM}), none owned: blocking", rung.key));
            Err(food_blocked(FOOD_ITEM))
        }
        None => Err(ScriptError::Failed(format!("ladder: no workable melee rung at {:?}", bot.position()))),
    }
}

#[cfg(test)]
mod tests {
    use super::{gate_items, holdings_from, plan_of};
    use lifescape_gamedata::melee_ladder::{rung_by_key, BRASS_KEY, EAT_FRACTION};

    /// The ladder's eat line is the fight loop's.
    #[test]
    fn ladder_eats_where_the_fight_eats() {
        assert_eq!(EAT_FRACTION, crate::kit::survival::EAT_FRACTION);
    }

    /// Exempt rungs keep the usual load and never require food; biting
    /// rungs carry the sized load and require it.
    #[test]
    fn plans_follow_the_food_rule() {
        let owned = holdings_from((4, 7), (12, 3), 0, false, false);
        assert_eq!((owned.food_count, owned.best_heal), (16, 7), "pack plus bank, best heal");
        let cows = rung_by_key("cows").expect("cows");
        let plan = plan_of(&cows, &[(3253, 3270, 0, 6)], None, &owned);
        assert!(!plan.food_required);
        assert_eq!(plan.food_target, crate::kit::survival::FOOD_TARGET as u32);
        assert_eq!(plan.clusters, Some(vec![(3253, 3270, 6)]));
        let guards = rung_by_key("guards").expect("guards");
        let plan = plan_of(&guards, &[(2962, 3386, 0, 8)], Some(9), &owned);
        assert!(plan.food_required);
        assert_eq!(plan.food_target, 9);
        assert!(plan.keep.is_empty(), "open gate keeps nothing");
    }

    /// Gate items ride every trip: the key for the giants, ten coins for
    /// the toll from the west, nothing once east.
    #[test]
    fn gates_keep_their_items() {
        let west = holdings_from((5, 3), (0, 0), 50, true, false);
        let giants = rung_by_key("hill_giants").expect("giants");
        assert_eq!(gate_items(&giants, &west), vec![(BRASS_KEY.to_string(), 1)]);
        let warriors = rung_by_key("al_kharid_warriors").expect("warriors");
        assert_eq!(gate_items(&warriors, &west), vec![("coins".to_string(), 10)]);
        let east = holdings_from((5, 3), (0, 0), 0, false, true);
        assert!(gate_items(&warriors, &east).is_empty(), "east of the fence: no toll");
    }
}
