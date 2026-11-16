//! Priced magic, D18 spells (user 2026-11-16: every F2P spell is a
//! method the brain can pick): teleports, curses, bones to bananas and
//! crumble undead join the combat spell and alchemy in `rank_magic`, each
//! priced like the others — its runes per cast (an elemental staff pays
//! its own), its other inputs (bones), its product (bananas) at what the
//! market absorbs, the bot's time. Same score and ranking as `training`.

use crate::spell_methods::{curse_method, debuff_spells, spell_spots, teleport_spells, BANANA, BANANA_BONES_PER_CAST};
use crate::training::{actions_for_xp, cost_input_of, pick_of, TrainPick, TrainPrices, BANK_TRIP_TICKS, FIGHT_CAST_TICKS};

/// Ticks of one teleport (`player_teleport_normal`: `p_delay(2)`, the
/// jump, the next click); runes stack, one load serves the climb.
pub const TELEPORT_TICKS: f64 = 5.0;
/// Ticks of one curse: the 5-tick action delay, the walk to a fresh foe
/// (a lowered one refuses the next cast) and the retaliating one
/// finished in melee (`player_magic.rs2` `npc_retaliate`; a cow's 8 HP
/// at a mage's low strength, ~5 swings of 4 ticks).
pub const CURSE_TICKS: f64 = 25.0;
/// Ticks of one bones to bananas: its `p_delay(2)` plus the bank trip
/// that brings the next pack of bones (one cast converts them all).
pub const BANANA_TICKS: f64 = 3.0 + BANK_TRIP_TICKS;

/// Lowest magic level with a choice beyond the strikes (confuse, 3):
/// a magic climb is priced from there (`training::priced_at_resolve`).
pub fn magic_priced_from() -> u8 {
    let levels = debuff_spells().into_iter().chain(teleport_spells()).chain(std::iter::once(crate::spell_methods::BONES_TO_BANANAS.to_string())).filter_map(|spell| {
        lifescape_gamedata::spells::by_name(&spell).into_iter().next().map(|row| u8::try_from(row.level.clamp(1, 99)).unwrap_or(99))
    });
    levels.min().unwrap_or(crate::training::ALCH_FROM_LEVEL)
}

/// Runes (and bones) the D18 spells open below `target` burn: a priced
/// magic climb keeps them between launches (`training::kept_inputs`).
pub fn kept_spell_inputs(target: u8) -> Vec<String> {
    let below = |method: &str| crate::method_way::method_way(method).and_then(|(_, way)| way.skill).is_some_and(|(_, level)| level < i64::from(target));
    let mut methods: Vec<String> = teleport_spells().into_iter().map(|spell| format!("teleport:{spell}")).collect();
    methods.extend(debuff_spells().into_iter().map(|spell| curse_method(&spell, 1)));
    methods.push(format!("bananas:{}", crate::spell_methods::BANANA_BONES));
    let mut out: Vec<String> = Vec::new();
    for method in methods.iter().filter(|method| below(method)) {
        for (input, _) in crate::method_way::method_way(method).map(|(_, way)| way.inputs).unwrap_or_default() {
            if !out.contains(&input) {
                out.push(input);
            }
        }
    }
    for spell in lifescape_gamedata::spells::all_spells().into_iter().filter(|row| !row.members && row.deals_damage() && row.level < i64::from(target) && lifescape_gamedata::spells::target_param(&row.spell).is_some()) {
        for (rune, _) in crate::method_way::spell_runes(&spell.spell) {
            if !out.contains(&rune) {
                out.push(rune);
            }
        }
    }
    out
}

/// One per-cast option: its way's inputs priced (runes, bones), the
/// product it yields (`made` units of `product` per cast), `ticks` a cast.
fn cast_option(method: String, level: u8, xp_needed: f64, ticks: f64, product: Option<(&str, f64)>, prices: &TrainPrices) -> Option<TrainPick> {
    let (_, way) = crate::method_way::method_way(&method)?;
    if way.skill.as_ref().is_none_or(|(_, need)| *need > i64::from(level)) {
        return None;
    }
    let xp_each = crate::sizing::xp_per_action(&method, level)?;
    let casts = actions_for_xp(xp_needed, xp_each);
    let mut inputs_each = 0.0;
    let mut missing = Vec::new();
    for (input, per) in &way.inputs {
        let per = (*per).max(1);
        let costed = cost_input_of(input, casts * per, per, "magic", prices)?;
        inputs_each += per as f64 * costed.each;
        if costed.short {
            missing.push(input.clone());
        }
    }
    let value = product.map(|(item, made)| (prices.product_value)(item, (casts as f64 * made).ceil() as i64).max(0.0) * made).unwrap_or(0.0);
    pick_of(method, 0, missing, ticks * prices.coin_rate + inputs_each - value, xp_each, ticks, prices.coin_rate)
}

/// Fight option of a spell with a target param on the surface camps it
/// fits (crumble undead on the Wilderness skeletons from 39).
fn restricted_fight_options(level: u8, xp_needed: f64, prices: &TrainPrices) -> Vec<TrainPick> {
    let mut out = Vec::new();
    let restricted = lifescape_gamedata::spells::all_spells().into_iter().filter(|row| !row.members && row.deals_damage() && row.level <= i64::from(level) && lifescape_gamedata::spells::target_param(&row.spell).is_some());
    for spell in restricted {
        let Some(spot) = spell_spots(&spell.spell).into_iter().next() else { continue };
        let method = format!("fight:magic/{}/{spot}", spell.spell);
        let Some(xp_each) = crate::sizing::xp_per_action(&method, level) else { continue };
        let casts = actions_for_xp(xp_needed, xp_each);
        let mut runes_each = 0.0;
        let mut missing = Vec::new();
        let mut feasible = true;
        for (rune, per_cast) in crate::method_way::spell_runes(&spell.spell) {
            let Some(costed) = cost_input_of(&rune, casts * per_cast, per_cast, "magic", prices) else {
                feasible = false;
                break;
            };
            runes_each += per_cast as f64 * costed.each;
            if costed.short {
                missing.push(rune);
            }
        }
        if feasible {
            out.extend(pick_of(method, 0, missing, FIGHT_CAST_TICKS * prices.coin_rate + runes_each, xp_each, FIGHT_CAST_TICKS, prices.coin_rate));
        }
    }
    out
}

/// Every D18 option open at `level` toward `target`, priced (unsorted:
/// `rank_magic` sorts them with the combat spell and alchemy).
pub fn spell_options(level: u8, target: u8, prices: &TrainPrices) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    for spell in teleport_spells() {
        out.extend(cast_option(format!("teleport:{spell}"), level, xp_needed, TELEPORT_TICKS, None, prices));
    }
    for spell in debuff_spells() {
        out.extend(cast_option(curse_method(&spell, level), level, xp_needed, CURSE_TICKS, None, prices));
    }
    let bananas = format!("bananas:{}", crate::spell_methods::BANANA_BONES);
    out.extend(cast_option(bananas, level, xp_needed, BANANA_TICKS, Some((BANANA, BANANA_BONES_PER_CAST as f64)), prices));
    out.extend(restricted_fight_options(level, xp_needed, prices));
    out
}

#[cfg(test)]
#[path = "training_spells_tests.rs"]
mod tests;
