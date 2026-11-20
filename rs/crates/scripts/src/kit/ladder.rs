//! Live side of the combat ladder (`gamedata::melee_ladder`): reads the
//! bot (base levels, worn bonuses, the run's style — melee, ranged with
//! its bow and arrows, magic with its spell —, food and coins in pack and
//! bank, the brass key, the toll flank), asks the ladder and turns its
//! pick into a spot plus a `HuntPlan`. A foodless bot whose best rung
//! bites gets the typed `Blocked` the brain turns into a food obtain.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use knowledge::melee_ladder::{food_to_ask, Gate, Holdings, Pick, Rung, BOAT_COINS, BRASS_KEY, TOLL_COINS};
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

/// Sum of one attack bonus (`range`, `magic`) over the worn gear.
fn worn_attack(worn: &[knowledge::equipment::Wearable], kind: &str) -> i64 {
    worn.iter().flat_map(|piece| piece.attack.iter()).filter(|(name, _)| name == kind).map(|(_, bonus)| *bonus).sum()
}

/// Ranged strength of the worn arrows (`BRONZE_ARROW_STRENGTH` when no
/// arrow is worn: the gear plan wears them at the bank).
pub fn arrow_strength(worn: &[knowledge::equipment::Wearable]) -> i64 {
    worn.iter().map(|piece| piece.rangebonus).filter(|bonus| *bonus >= 0).max().unwrap_or(knowledge::melee_math::BRONZE_ARROW_STRENGTH)
}

/// Max hit and base xp of a cast of `spell` (the spell dump); wind
/// strike's when the spell is unknown.
pub fn cast_of(spell: Option<&str>) -> (i64, f64) {
    let row = knowledge::spells::by_name(spell.unwrap_or("wind_strike")).into_iter().chain(knowledge::spells::by_name("wind_strike")).find(|row| row.maxhit.is_some());
    row.map(|row| (row.maxhit.unwrap_or(0), row.xp as f64 / 10.0)).unwrap_or((0, 0.0))
}

/// The fighter as the ladder rates it: base levels, max hitpoints,
/// combat level, the worn bonuses and the run's style (`combat_style`:
/// ranged shoots with its level, bow and arrows; magic casts the run's
/// spell, `combat_spell`; anything else swings).
pub fn fighter(bot: &Bot) -> Fighter {
    let (attack, defence, strength, hitpoints) = (base_level(bot, 0, 1), base_level(bot, 1, 1), base_level(bot, 2, 1), base_level(bot, 3, 10));
    let combat = bot.combat_level().map(i64::from).unwrap_or(3);
    let worn = worn_gear(bot);
    let melee = Fighter::naked(attack, strength, defence, hitpoints, combat).wearing(&worn);
    match bot.combat_style.as_str() {
        "ranged" => melee.shooting(base_level(bot, 4, 1), worn_attack(&worn, "range"), arrow_strength(&worn)),
        "magic" => {
            let (max_hit, cast_xp) = cast_of(bot.combat_spell.as_deref());
            melee.casting(base_level(bot, 6, 1), worn_attack(&worn, "magic"), max_hit, cast_xp)
        }
        _ => melee,
    }
}

/// Rungs the run's style cannot hunt: a magic run's spell must reach
/// every NPC of the camp (`melee_ladder::unfit_for_spell`).
pub fn style_unfit(bot: &Bot) -> Vec<&'static str> {
    if bot.combat_style != "magic" {
        return Vec::new();
    }
    knowledge::melee_ladder::unfit_for_spell(bot.combat_spell.as_deref().unwrap_or("wind_strike"))
}

/// Food a foodless bot asks the brain for: the biggest heal its max
/// hitpoints use (`melee_ladder::food_to_ask`).
pub fn asked_food(bot: &Bot) -> &'static str {
    food_to_ask(base_level(bot, 3, 10))
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
    let east_of_toll = crate::kit::bank::toll_gate_x(bot).is_some() && lifescape_gamedata::toll::behind(i64::from(bot.world.x), i64::from(bot.world.z), i64::from(bot.world.level));
    holdings_from(food_in(bot, pack.iter().copied()), food_in(bot, bank.iter().copied()), coins, brass_key, east_of_toll)
}

/// Items that ride every trip (name, units): what the rung's gate needs,
/// plus its shortcut key when the bot owns it (the brass key: the nav
/// takes the house door only with the key in the pack).
pub fn gate_items(rung: &Rung, holdings: &Holdings) -> Vec<(String, u32)> {
    let mut out = match rung.gate {
        Gate::Open => Vec::new(),
        Gate::Toll if holdings.east_of_toll => Vec::new(),
        Gate::Toll => vec![("coins".to_string(), TOLL_COINS)],
        // The sailors and the customs officer take the fare from the
        // pack, out and back: every trip leaves the bank with both.
        Gate::Boat => vec![("coins".to_string(), BOAT_COINS)],
    };
    if let Some(key) = rung.shortcut_key.filter(|key| *key == BRASS_KEY && holdings.brass_key) {
        out.push((key.to_string(), 1));
    }
    out
}

/// The plan of one hunt pick (pure: tests drive it without a bot).
pub fn plan_of(rung: &Rung, clusters: &[(i64, i64, i64, usize)], food: Option<u32>, holdings: &Holdings) -> HuntPlan {
    HuntPlan {
        clusters: Some(clusters.iter().map(|(x, z, _, spawns)| (*x as i32, *z as i32, *spawns)).collect()),
        food_target: food.unwrap_or(crate::kit::survival::FOOD_TARGET as u32),
        food_required: food.is_some(),
        keep: gate_items(rung, holdings),
        stop_level: None,
        stop_want: None,
    }
}

/// Harmless-spot rule (user 2026-11-17), shared with the planner's
/// money choice (`gamedata::melee_ladder::spot_food_required`).
pub use knowledge::melee_ladder::{harmless, HARMLESS_HIT_SHARE};

/// True when an ordered spot (`melee goblins`, a drop farm, a ranged or
/// magic hunt, a filler) must not be fought without food: every spot but
/// the exempt rungs (cows, chickens; user rule "food everywhere but cows
/// and chickens", applied to ordered spots since 2026-11-17: live, 57
/// `fight:melee/men` runs ended in a death, foodless) and the harmless
/// ones for this bot's base hitpoints `max_hp` (men, max hit 1, for a
/// 30-HP bot; server stats).
pub fn ordered_food_required(spot_key: &str, max_hp: i64) -> bool {
    knowledge::melee_ladder::spot_food_required(spot_key, max_hp)
}

/// Load of an ordered spot: the ladder's sized load when the spot is a
/// rated rung (burn over a trip plus the reserve), else the usual
/// `FOOD_TARGET`.
pub fn ordered_food_load(rated: &[knowledge::melee_ladder::Rated], spot_key: &str) -> u32 {
    rated.iter().find(|entry| entry.rung.key == spot_key).map(|entry| entry.food).filter(|food| *food > 0).unwrap_or(crate::kit::survival::FOOD_TARGET as u32)
}

/// The hunt plan of an ordered spot: ranked clusters (no ladder order),
/// the food rule and load above, no kept items.
pub fn ordered_plan(bot: &mut Bot, spot_key: &str) -> HuntPlan {
    if !ordered_food_required(spot_key, base_level(bot, 3, 10)) {
        return HuntPlan::plain();
    }
    let rated = knowledge::melee_ladder::rate(&fighter(bot), &holdings(bot));
    HuntPlan { food_target: ordered_food_load(&rated, spot_key), food_required: true, ..HuntPlan::plain() }
}

/// Ask the ladder (rungs in `excluded` left out: killers of this run).
/// Hunt: the spot and its plan (the toll opened when the rung needs it).
/// NeedFood: typed `Blocked` on `asked_food`. Nothing workable: Failed.
pub fn pick(bot: &mut Bot, excluded: &[&str]) -> Result<(knowledge::combat_spots::CombatSpot, HuntPlan), ScriptError> {
    let melee = fighter(bot);
    let owned = holdings(bot);
    let seed = knowledge::melee_ladder::seed_of(&bot.username);
    let mut left_out: Vec<&str> = excluded.to_vec();
    left_out.extend(style_unfit(bot));
    let choice = knowledge::melee_ladder::choose_excluding(&melee, &owned, seed, &left_out);
    bot.log(&format!(
        "ladder ({:?}): atk={} str={} def={} hp={} cb={} bonus atk={:?} str={} def={:?} food={}x{} coins={} key={} -> {choice:?}",
        melee.offense,
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
            if rung.gate == Gate::Boat {
                // The boat is part of the map: the pack's coins are its
                // only gate (`gate_items` keeps the fare out and back).
                bot.log(&format!("ladder: {} behind the Port Sarim boat ({BOAT_COINS} coins kept)", rung.key));
            }
            Ok((spot, plan_of(&rung, &clusters, food, &owned)))
        }
        Some(Pick::NeedFood { rung, count }) => {
            let food = asked_food(bot);
            bot.log(&format!("ladder: {} needs food ({count} {food}), none owned: blocking", rung.key));
            Err(food_blocked(food))
        }
        None => Err(ScriptError::Failed(format!("ladder: no workable melee rung at {:?}", bot.position()))),
    }
}

#[cfg(test)]
mod tests {
    use super::{gate_items, holdings_from, ordered_food_load, ordered_food_required, plan_of};
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

    /// Ordered spots follow the food rule too: only cows and chickens
    /// are fought without food; a rated rung carries its sized load.
    #[test]
    fn ordered_spots_need_food_but_cows_and_chickens() {
        assert!(!ordered_food_required("cows", 30) && !ordered_food_required("chickens", 10));
        for spot in ["hill_giants", "wild_skeletons", "barbarians"] {
            assert!(ordered_food_required(spot, 30), "{spot}");
        }
        assert!(ordered_food_required("goblins", 10), "goblins bite a 10-HP bot");
        // User 2026-11-17: a harmless spot (NPC max hit under 5 % of the
        // bot's base hitpoints) is fought foodless, like cows.
        assert_eq!(lifescape_gamedata::melee_math::npc_max_hit("man"), Some(1), "men hit 1");
        assert!(super::harmless(1, 30), "max hit 1 for 30 HP: harmless");
        assert!(!super::harmless(5, 30), "max hit 5 for 30 HP: food");
        assert!(!ordered_food_required("men", 30), "men for a 30-HP bot: no food");
        assert!(ordered_food_required("men", 10), "men for a 10-HP bot: food (1 >= 0.5)");
        let melee = lifescape_gamedata::melee_math::Fighter::naked(40, 40, 40, 40, 46);
        let rated = lifescape_gamedata::melee_ladder::rate(&melee, &holdings_from((0, 0), (20, 7), 0, false, false));
        let giants = ordered_food_load(&rated, "hill_giants");
        assert!(giants > 0, "sized load");
        assert_eq!(ordered_food_load(&rated, "wild_skeletons"), crate::kit::survival::FOOD_TARGET as u32, "unrated: the usual 10");
    }

    /// Gate items ride every trip: the owned key for the giants (a
    /// shortcut, never required), ten coins for the toll from the west,
    /// nothing once east.
    #[test]
    fn gates_keep_their_items() {
        let west = holdings_from((5, 3), (0, 0), 50, true, false);
        let giants = rung_by_key("hill_giants").expect("giants");
        assert_eq!(gate_items(&giants, &west), vec![(BRASS_KEY.to_string(), 1)], "owned key rides as the shortcut");
        let keyless = holdings_from((5, 3), (0, 0), 50, false, false);
        assert!(gate_items(&giants, &keyless).is_empty(), "no key: the trapdoor, nothing kept");
        let warriors = rung_by_key("al_kharid_warriors").expect("warriors");
        assert_eq!(gate_items(&warriors, &west), vec![("coins".to_string(), 10)]);
        let east = holdings_from((5, 3), (0, 0), 0, false, true);
        assert!(gate_items(&warriors, &east).is_empty(), "east of the fence: no toll");
        // User 2026-11-20: the volcano demons ride the boat both ways,
        // 30 coins each from the pack (`sailors.rs2`, `customs_officer.rs2`).
        let demons = rung_by_key("lesser_demons").expect("demons");
        assert_eq!(gate_items(&demons, &west), vec![("coins".to_string(), 60)]);
    }

    /// Archers shoot their worn arrows (bronze when none is worn); a
    /// mage's cast reads the spell dump.
    #[test]
    fn styles_read_their_own_gear() {
        let rune = lifescape_gamedata::equipment::by_name("rune_arrow").expect("rune arrow");
        let bow = lifescape_gamedata::equipment::by_name("shortbow").expect("bow");
        assert_eq!(super::arrow_strength(&[bow.clone(), rune.clone()]), rune.rangebonus);
        assert_eq!(super::arrow_strength(&[bow]), lifescape_gamedata::melee_math::BRONZE_ARROW_STRENGTH);
        let (blast, blast_xp) = super::cast_of(Some("fire_blast"));
        let (strike, _) = super::cast_of(None);
        assert!(blast > strike && strike > 0 && blast_xp > 0.0, "fire blast {blast} vs wind strike {strike}");
    }
}
