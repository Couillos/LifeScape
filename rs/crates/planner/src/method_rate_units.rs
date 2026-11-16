//! Spell and alchemy actions of the rates prior (`method_rate`, split
//! for rule 4): timings from the server spell scripts (rev 274).
//! - high alchemy `p_delay(3)`: 5 ticks; low alchemy `p_delay(1)`: 3
//!   (`alchemy.rs2`);
//! - teleports `p_delay(2)` then the jump: 3 ticks (`teleport.rs2`);
//! - curses: a magic attack, 5 ticks (`player_magic.rs2`);
//! - bones to bananas `p_delay(2)`: 4 ticks (`convert_bones.rs2`).

use crate::method_rate::{Action, PriorError};

/// Cause of a consumed recipe input: a rune some spell burns, else an
/// input proper.
pub fn input_cause(item: &str) -> &'static str {
    let rune = lifescape_gamedata::spells::all_spells().iter().any(|spell| spell.runes.iter().any(|(name, _)| name == item));
    if rune { "rune" } else { "input" }
}

/// One cast of alchemy on `item` at `level` (the spell the level casts).
pub fn alch_action(item: &str, level: u8) -> Result<Action, PriorError> {
    let spell = crate::alchemy::alch_spell(level).ok_or_else(|| PriorError::NoOdds(format!("alchemy below magic 21 ({item})")))?;
    let coins = crate::alchemy::alch_value(item, spell.high).ok_or_else(|| PriorError::NoOdds(format!("alchemy value of {item}")))?;
    let ticks = if spell.high { 5.0 } else { 3.0 };
    let mut flows: Vec<(String, &'static str, f64)> = vec![(item.to_string(), "input", -1.0), ("coins".to_string(), "product", coins as f64)];
    flows.extend(spell.runes.iter().map(|(rune, count)| (rune.clone(), "rune", -(*count as f64))));
    Ok(Action { ticks, success: 1.0, flows, units: vec![("input", item.to_string(), 1.0)], slots: 1.0, sites: Vec::new(), banks: true })
}

/// A D18 per-cast method (teleport, curse, bones to bananas); `None`
/// for any other method.
pub fn spell_action(method: &str) -> Result<Option<Action>, PriorError> {
    if method.starts_with("fight:") {
        return Ok(None);
    }
    let Some(spell) = crate::spell_methods::cast_spell(method) else { return Ok(None) };
    let (product, way) = crate::spell_methods::spell_way(method).ok_or_else(|| PriorError::NoWay(method.to_string()))?;
    let (ticks, banks) = if method.starts_with("teleport:") {
        (3.0, false)
    } else if method.starts_with("curse:") {
        (5.0, false)
    } else {
        (4.0, true)
    };
    let mut flows: Vec<(String, &'static str, f64)> = way.inputs.iter().map(|(item, count)| (item.clone(), input_cause(item), -(*count as f64))).collect();
    let mut slots = 0.0;
    if !product.is_empty() {
        let made = way.qty.map(|(low, high)| (low + high) as f64 / 2.0).unwrap_or(1.0);
        flows.push((product, "product", made));
        slots = made;
    }
    Ok(Some(Action { ticks, success: 1.0, flows, units: vec![("cast", spell, 1.0)], slots, sites: way.sites, banks }))
}
