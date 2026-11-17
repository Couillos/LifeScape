//! Priced magic, D18 spells (user 2026-11-16: every F2P spell is a
//! method the brain can pick): teleports, curses, bones to bananas and
//! crumble undead join the combat spell and alchemy in `rank_magic`, each
//! priced like the others — its runes per cast (an elemental staff pays
//! its own), its other inputs (bones), its product (bananas) at what the
//! market absorbs, the bot's time — all per XP from the spell's yield
//! (R3.3, `training::rated_option`: measured, else the server timing of
//! `method_rate_units`). Same score and ranking as `training`.

use crate::spell_methods::{curse_method, debuff_spells, spell_spots, teleport_spells};
use crate::training::{rated_option, TrainPick, TrainPrices};

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

/// One per-cast option open at `level`: priced from its rate (its runes
/// and bones per XP, its product, the bot's time per XP).
fn cast_option(method: String, level: u8, xp_needed: f64, prices: &TrainPrices) -> Option<TrainPick> {
    let (_, way) = crate::method_way::method_way(&method)?;
    if way.skill.as_ref().is_none_or(|(_, need)| *need > i64::from(level)) {
        return None;
    }
    rated_option(&method, "magic", level, xp_needed, prices)
}

/// Fight option of a spell with a target param on the surface camps it
/// fits (crumble undead on the Wilderness skeletons from 39).
fn restricted_fight_options(level: u8, xp_needed: f64, prices: &TrainPrices) -> Vec<TrainPick> {
    let mut out = Vec::new();
    let restricted = lifescape_gamedata::spells::all_spells().into_iter().filter(|row| !row.members && row.deals_damage() && row.level <= i64::from(level) && lifescape_gamedata::spells::target_param(&row.spell).is_some());
    for spell in restricted {
        let Some(spot) = spell_spots(&spell.spell).into_iter().next() else { continue };
        out.extend(rated_option(&format!("fight:magic/{}/{spot}", spell.spell), "magic", level, xp_needed, prices));
    }
    out
}

/// Every D18 option open at `level` toward `target`, priced (unsorted:
/// `rank_magic` sorts them with the combat spell and alchemy).
pub fn spell_options(level: u8, target: u8, prices: &TrainPrices) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    for spell in teleport_spells() {
        out.extend(cast_option(format!("teleport:{spell}"), level, xp_needed, prices));
    }
    for spell in debuff_spells() {
        out.extend(cast_option(curse_method(&spell, level), level, xp_needed, prices));
    }
    out.extend(cast_option(format!("bananas:{}", crate::spell_methods::BANANA_BONES), level, xp_needed, prices));
    out.extend(restricted_fight_options(level, xp_needed, prices));
    out
}

#[cfg(test)]
#[path = "training_spells_tests.rs"]
mod tests;
