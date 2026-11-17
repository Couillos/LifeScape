//! Meals (user 2026-11-17): when a fight eats, what it eats, how it
//! heals at the bank and which banked food rides a hunt.
//! - Eat line: `max − best held heal` (that bite wastes nothing), never
//!   under the emergency line; a bot never waits near 15 % of its health.
//! - Emergency line: two of the foe's max hits (`SAFETY_HITS`: one hit
//!   may land during the 3-tick eat, the next must not kill), at least
//!   `EMERGENCY_FRACTION` of max hitpoints. Under it the biggest heal
//!   goes, waste or not; a foodless biting hunt leaves for the bank.
//! - Above the emergency line no hitpoint is wasted: the biggest heal
//!   that fits the missing hitpoints, nothing when none fits.
//! - At the combat bank the bot eats back to full health with the same
//!   no-waste pick before it walks out (`eat_to_full`).
//! - Food that rides a hunt fits the bot's health: heals up to
//!   `melee_ladder::food_room` (half its max hitpoints) first, best heal
//!   first; a bigger heal only when nothing fitting is banked.
//! Server truth: heals from `foods.json` (`gamedata::food`), alcoholic
//! drinks never food; max hits from `npc_combat` (`melee_math`).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::survival::{hitpoints, max_hitpoints};
use crate::ScriptError;

/// Foe max hits a fight keeps in hand before it must eat.
pub const SAFETY_HITS: i64 = 2;
/// The emergency line never sits under this share of max hitpoints.
pub const EMERGENCY_FRACTION: f64 = 0.35;
/// Foe max hit assumed when the spot's NPCs have no melee block.
pub const UNKNOWN_FOE_MAX_HIT: i64 = 3;

/// Hitpoints at or under which the fight eats its biggest heal, waste or
/// not (and a foodless biting hunt leaves): two foe max hits, at least
/// `EMERGENCY_FRACTION` of the max, always under full health.
pub fn emergency_line(max_hp: u8, foe_max_hit: i64) -> u8 {
    let by_hits = SAFETY_HITS.saturating_mul(foe_max_hit.max(1));
    let by_share = (f64::from(max_hp) * EMERGENCY_FRACTION).ceil() as i64;
    by_hits.max(by_share).clamp(1, i64::from(max_hp.max(2)) - 1) as u8
}

/// Hitpoints at or under which the fight eats: `max − best_heal` (the
/// best held bite then wastes nothing), never under the emergency line;
/// no food held: the emergency line alone.
pub fn eat_line(max_hp: u8, best_heal: Option<i64>, foe_max_hit: i64) -> u8 {
    let emergency = emergency_line(max_hp, foe_max_hit);
    match best_heal.filter(|heal| *heal > 0) {
        Some(heal) => (i64::from(max_hp) - heal).clamp(i64::from(emergency), i64::from(max_hp.max(2)) - 1) as u8,
        None => emergency,
    }
}

/// Index of the food to eat among `heals` at `hp`/`max_hp`: under the
/// emergency line the biggest heal; otherwise the biggest heal that fits
/// the missing hitpoints (no waste), `None` when none fits.
pub fn pick_food(hp: u8, max_hp: u8, heals: &[i64], emergency: bool) -> Option<usize> {
    let indexed = heals.iter().copied().enumerate().filter(|(_, heal)| *heal > 0);
    if emergency {
        return indexed.max_by_key(|(_, heal)| *heal).map(|(index, _)| index);
    }
    let missing = i64::from(max_hp.saturating_sub(hp));
    indexed.filter(|(_, heal)| *heal <= missing).max_by_key(|(_, heal)| *heal).map(|(index, _)| index)
}

/// Bites that heal `missing` hitpoints without waste from `stock` (heal,
/// units): biggest fitting heal first. Returns the units eaten per heal.
pub fn heal_plan(missing: i64, stock: &[(i64, u32)]) -> Vec<(i64, u32)> {
    let mut left = missing.max(0);
    let mut sorted: Vec<(i64, u32)> = stock.iter().copied().filter(|(heal, units)| *heal > 0 && *units > 0).collect();
    sorted.sort_by(|left_entry, right_entry| right_entry.0.cmp(&left_entry.0));
    let mut out = Vec::new();
    for (heal, units) in sorted {
        let bites = (left / heal).min(i64::from(units));
        if bites > 0 {
            out.push((heal, bites as u32));
            left -= bites * heal;
        }
    }
    out
}

/// Order of the banked foods a hunt withdraws for a bot of `max_hp`:
/// heals within `food_room` first (best first), then the oversized ones
/// smallest first (least waste). Sort key, lower first.
pub fn carry_rank(heal: i64, max_hp: u8) -> (u8, i64) {
    if heal <= lifescape_gamedata::melee_ladder::food_room(i64::from(max_hp)) {
        (0, -heal)
    } else {
        (1, heal)
    }
}

/// Biggest melee max hit among a spot's NPCs (`UNKNOWN_FOE_MAX_HIT`
/// when none has a melee block).
pub fn foe_max_hit(npc_names: &[&str]) -> i64 {
    npc_names.iter().filter_map(|name| lifescape_gamedata::melee_math::npc_max_hit(name)).max().unwrap_or(UNKNOWN_FOE_MAX_HIT)
}

/// Edible stacks of the pack: (slot, obj, heal).
pub fn pack_foods(bot: &Bot) -> Vec<(u8, u16, i64)> {
    bot.backpack()
        .1
        .into_iter()
        .filter_map(|(slot, (held, _))| {
            let heal = (bot.tables.obj_name)(held).map(|name| lifescape_gamedata::food::heal(&name)).unwrap_or(0);
            (heal > 0).then_some((slot, held, heal))
        })
        .collect()
}

/// Food the bot owns, pack plus the open bank view: (heal, units).
pub fn food_stock(bot: &Bot) -> Vec<(i64, u32)> {
    let heal_of = |obj: u16| (bot.tables.obj_name)(obj).map(|name| lifescape_gamedata::food::heal(&name)).unwrap_or(0);
    let mut stock: Vec<(i64, u32)> = Vec::new();
    let pack = bot.backpack().1.into_iter().map(|(_, stack)| stack);
    for (obj, units) in pack.chain(bot.bank_items()) {
        let heal = heal_of(obj);
        if heal > 0 {
            stock.push((heal, units));
        }
    }
    stock
}

/// Eat one bite of the pack's food at index `pick` of `foods`.
fn bite(bot: &mut Bot, foods: &[(u8, u16, i64)], pick: usize) -> Result<bool, ScriptError> {
    let Some((slot, held, _)) = foods.get(pick).copied() else { return Ok(false) };
    let Some(pack_com) = bot.backpack().0 else { return Ok(false) };
    bot.op_held(1, held, slot, pack_com)?;
    bot.pump(3)?;
    Ok(true)
}

/// What a fight turn decided about food.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meal {
    /// Above the eat line, or nothing fits without waste yet.
    Held,
    /// One bite eaten.
    Ate,
    /// At or under the emergency line with nothing edible in the pack.
    Starving,
}

/// The fight's meal turn (base max hitpoints, never the highest current
/// hitpoints seen: a fight started wounded ate at 13/26 on a 64-HP bot).
pub fn fight_meal(bot: &mut Bot, foe_max_hit: i64) -> Result<Meal, ScriptError> {
    let (hp, max_hp) = (hitpoints(bot), max_hitpoints(bot).max(hitpoints(bot)));
    let foods = pack_foods(bot);
    let best = foods.iter().map(|(_, _, heal)| *heal).max();
    if hp > eat_line(max_hp, best, foe_max_hit) {
        return Ok(Meal::Held);
    }
    let emergency = hp <= emergency_line(max_hp, foe_max_hit);
    if foods.is_empty() {
        return Ok(if emergency { Meal::Starving } else { Meal::Held });
    }
    let heals: Vec<i64> = foods.iter().map(|(_, _, heal)| *heal).collect();
    match pick_food(hp, max_hp, &heals, emergency) {
        Some(pick) if bite(bot, &foods, pick)? => Ok(Meal::Ate),
        _ => Ok(Meal::Held),
    }
}

/// Eat back to full health without wasting a hitpoint (the combat bank,
/// before walking out). Returns the bites eaten.
pub fn eat_to_full(bot: &mut Bot) -> Result<u32, ScriptError> {
    let mut bites = 0u32;
    // One bite per pack slot at most: bounded even if a bite is refused.
    for _ in 0..28 {
        let (hp, max_hp) = (hitpoints(bot), max_hitpoints(bot));
        if hp == 0 || hp >= max_hp {
            break;
        }
        let foods = pack_foods(bot);
        let heals: Vec<i64> = foods.iter().map(|(_, _, heal)| *heal).collect();
        let Some(pick) = pick_food(hp, max_hp, &heals, false) else { break };
        if !bite(bot, &foods, pick)? {
            break;
        }
        bites += 1;
    }
    if bites > 0 {
        bot.log(&format!("ate {bites} at the bank: hp {}/{}", hitpoints(bot), max_hitpoints(bot)));
    }
    Ok(bites)
}

#[cfg(test)]
mod tests {
    use super::{carry_rank, eat_line, emergency_line, foe_max_hit, heal_plan, pick_food};

    /// User 2026-11-17: never wait near 15 %. The emergency line keeps two
    /// foe max hits in hand, at least 35 % of the health.
    #[test]
    fn emergency_line_keeps_two_max_hits() {
        assert_eq!(emergency_line(70, 3), 25, "35 % of 70 beats 2 x 3");
        assert_eq!(emergency_line(70, 20), 40, "a 20-hitter: 40 in hand");
        assert_eq!(emergency_line(10, 1), 4);
        assert_eq!(emergency_line(10, 30), 9, "always under full health");
    }

    /// The eat line is max minus the best held heal (no waste), never
    /// under the emergency line; no food: the emergency line.
    #[test]
    fn eat_line_wastes_nothing_and_never_waits_too_long() {
        assert_eq!(eat_line(55, Some(12), 3), 43, "lobster at 43/55 heals exactly to 55");
        assert_eq!(eat_line(70, Some(3), 3), 67, "shrimps: eat each 3 missing");
        assert_eq!(eat_line(20, Some(14), 2), 7, "swordfish on a 20-HP bot: the emergency line (35 %)");
        assert_eq!(eat_line(64, None, 5), 23, "foodless: the emergency line");
        let line = eat_line(64, Some(20), 3);
        assert!(f64::from(line) / 64.0 > 0.15, "never near 15 %: {line}");
    }

    /// 50/55 with a lobster and a trout: the trout would waste 2, the
    /// lobster 7: nothing fits, nothing eaten; at 43/55 the lobster.
    #[test]
    fn the_pick_never_wastes_above_the_emergency_line() {
        let heals = [12, 7, 3];
        assert_eq!(pick_food(50, 55, &heals, false), Some(2), "5 missing: the shrimp (3), never the lobster");
        assert_eq!(pick_food(54, 55, &heals, false), None, "1 missing: nothing fits");
        assert_eq!(pick_food(43, 55, &heals, false), Some(0), "12 missing: the lobster");
        assert_eq!(pick_food(10, 55, &heals, true), Some(0), "emergency: the biggest heal");
        assert_eq!(pick_food(10, 55, &[], true), None);
    }

    /// Bank heal: 27 missing with lobsters and shrimps eats two lobsters
    /// and a shrimp, never a third lobster (9 wasted).
    #[test]
    fn heal_plan_fills_without_waste() {
        assert_eq!(heal_plan(27, &[(3, 10), (12, 5)]), vec![(12, 2), (3, 1)]);
        assert_eq!(heal_plan(25, &[(3, 10), (12, 5)]), vec![(12, 2)], "1 left: no shrimp wastes 2");
        assert_eq!(heal_plan(0, &[(3, 10)]), vec![]);
        assert_eq!(heal_plan(5, &[(12, 3)]), vec![], "nothing fits");
    }

    /// A 20-HP bot carries tuna (10) before swordfish (14); a 70-HP bot
    /// carries swordfish first.
    #[test]
    fn carried_food_fits_the_health() {
        let mut small = vec![14i64, 10, 3];
        small.sort_by_key(|heal| carry_rank(*heal, 20));
        assert_eq!(small, vec![10, 3, 14]);
        let mut big = vec![3i64, 14, 10];
        big.sort_by_key(|heal| carry_rank(*heal, 70));
        assert_eq!(big, vec![14, 10, 3]);
    }

    /// Spot max hits from the server stats; unknown NPCs: the default.
    #[test]
    fn spot_max_hit_reads_the_npcs() {
        assert!(foe_max_hit(&["giant"]) > foe_max_hit(&["man"]));
        assert_eq!(foe_max_hit(&["no_such_npc"]), super::UNKNOWN_FOE_MAX_HIT);
    }
}
