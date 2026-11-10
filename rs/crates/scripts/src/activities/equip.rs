//! Equip: wear the best available kit for a style (`equip [style]`).
//! The brain orders `equip:melee|rigged` after its obtain node; this
//! wears the best carried piece per slot through `kit::gear` (score,
//! wear, log). Banked pieces stay banked: the obtain leg delivers to
//! the pack, the bank flow stays with `prepare_at_bank` callers.
//!
//! Process: poste aucun · action équiper · interface néant ·
//! intrants kit porté · produits kit porté.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::gear::{available_gear, equip_plan, gear_plan};
use crate::kit::style::{fight_weapon_style, style_damage_key};
use crate::{ScriptError, ScriptResult};

/// Styles the brain orders (planner `EquipBest`); anything else is a
/// brain bug and fails loud, never a silent melee.
pub const STYLES: [&str; 3] = ["melee", "ranged", "magic"];

/// Wear the best carried kit: `equip [style]` (melee when omitted).
/// Returns the equipped slot count.
pub fn run_equip(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let style = args
        .first()
        .filter(|first| {
            first.parse::<u32>().is_err()
                && first.as_str() != "toll"
                && first.as_str() != "wild"
                && !first.starts_with("--")
        })
        .cloned()
        .unwrap_or_else(|| "melee".to_string());
    if !STYLES.contains(&style.as_str()) {
        return Err(ScriptError::Usage(format!("equip melee|ranged|magic, not {style:?} at {:?}", bot.position())));
    }
    let available = available_gear(bot);
    let plan = gear_plan(bot, &available, fight_weapon_style, style_damage_key);
    let equipped = equip_plan(bot, &plan)?;
    let mut slots: Vec<(&i64, &String)> = equipped.iter().collect();
    slots.sort();
    for (slot, name) in &slots {
        bot.log(&format!("equipped {name} (slot {slot}) for {style}"));
    }
    bot.log(&format!("ok equip {style}: {} slots", slots.len()));
    Ok(slots.len() as i32)
}

#[cfg(test)]
mod tests {
    use super::{run_equip, STYLES};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Unknown styles fail loud, never wear a silent melee.
    #[test]
    fn unknown_style_fails_loud() {
        let mut bot = offline_bot();
        let error = run_equip(&mut bot, &["bananas".to_string()]).expect_err("unknown style fails");
        assert!(matches!(error, crate::ScriptError::Usage(_)), "typed usage, got {error}");
    }

    /// Empty pack wires through gear_plan + equip_plan and reports zero
    /// slots (the obtain leg delivers; nothing carried means nothing to
    /// wear, not a crash).
    #[test]
    fn empty_pack_equips_nothing() {
        let mut bot = offline_bot();
        assert_eq!(run_equip(&mut bot, &[]).expect("empty pack is Ok(0)"), 0);
        assert_eq!(STYLES, ["melee", "ranged", "magic"], "brain styles");
    }
}
