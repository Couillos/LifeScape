//! Equip: wear the best available kit for a style (`equip [style]`).
//! The brain orders `equip:melee|rigged` after its obtain node; this
//! wears the best owned piece per slot through `kit::gear` (score,
//! wear, log). The brain counts the bank as owned (`unworn_upgrade`):
//! a planned piece that only the last known bank holds is fetched first
//! (one unit each, plus the arrows the planned bow takes), then worn
//! (live 2026-11-14 : ryan_park « equip ranged » portait casque et
//! flèches de fer, arc court et corps de cuir restés en banque).
//!
//! Process: poste aucun · action équiper · interface néant ·
//! intrants kit porté · produits kit porté.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::combat_bank::{kit_to_withdraw, style_ammo_ids};
use crate::kit::gear::{equip_logged, gear_plan, owned_gear, worn_snapshot};
use crate::kit::pack::{banked_usable, pack_total};
use crate::kit::style::{fight_weapon_style, style_damage_key};
use crate::{ScriptError, ScriptResult};

/// Styles the brain orders (planner `EquipBest`); anything else is a
/// brain bug and fails loud, never a silent melee.
pub const STYLES: [&str; 3] = ["melee", "ranged", "magic"];

/// Style the order names (`equip [style] [--node ID] ...`, melee when
/// omitted). Anything else is a brain bug and fails loud.
pub fn parse_style(args: &[String]) -> Result<String, ScriptError> {
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
        return Err(ScriptError::Usage(format!("equip melee|ranged|magic, not {style:?}")));
    }
    Ok(style)
}

/// Wear the best owned kit: `equip [style]` (melee when omitted).
/// Exit `Ok(0)` like every supervised script (the session maps any other
/// code to `failed`; the slot count rides in the log, never the code).
pub fn run_equip(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let style = parse_style(args)?;
    // The kit scores for the asked style (bows + arrows, staves, robes).
    bot.combat_style = style.clone();
    let plan = gear_plan(bot, &owned_gear(bot), fight_weapon_style, style_damage_key);
    let wants = banked_wants(bot, &plan);
    if !wants.is_empty() {
        let bank = crate::kit::bank::nearest_bank(bot)?;
        crate::kit::withdraw::bank_withdraw_counts(bot, &bank, &wants)?;
    }
    // One line per slot that changed, never the whole kit again.
    let changed = equip_logged(bot, &plan, &format!("equipped for {style}:"))?;
    bot.log(&format!("ok equip {style}: {} slots worn, {} changed", worn_snapshot(bot).len(), changed.len()));
    Ok(0)
}

/// Planned pieces only the known bank holds (one unit each) and, for a
/// ranged kit, the arrow stacks the planned bow takes when the pack and
/// the quiver hold none of them.
pub fn banked_wants(bot: &Bot, plan: &crate::kit::combat_bank::GearPlan) -> Vec<(u16, u32)> {
    let mut wants: Vec<(u16, u32)> = kit_to_withdraw(bot, plan).into_iter().filter(|obj| banked_usable(bot, *obj) > 0).map(|obj| (obj, 1)).collect();
    if bot.combat_style == "ranged" {
        let arrows = style_ammo_ids(bot, plan);
        let loaded = arrows.iter().any(|obj| pack_total(bot, *obj) > 0 || crate::kit::pack::is_worn(bot, *obj));
        if !loaded {
            wants.extend(arrows.into_iter().map(|obj| (obj, banked_usable(bot, obj))).filter(|(_, count)| *count > 0).take(1));
        }
    }
    wants
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

    /// Empty pack wires through gear_plan + equip_logged and reports zero
    /// slots (the obtain leg delivers; nothing carried means nothing to
    /// wear, not a crash).
    #[test]
    fn empty_pack_equips_nothing() {
        let mut bot = offline_bot();
        assert_eq!(run_equip(&mut bot, &[]).expect("empty pack is Ok(0)"), 0);
        assert_eq!(STYLES, ["melee", "ranged", "magic"], "brain styles");
    }

    /// ryan_park (banque fermée, vue connue) : `equip ranged` va chercher
    /// l'arc court et le corps de cuir, une unité chacun ; le carquois de
    /// fer porté suffit, sinon la pile de flèches de bronze sort aussi.
    #[test]
    fn equip_ranged_fetches_the_banked_bow() {
        use std::collections::HashMap;
        let mut bot = offline_bot();
        bot.combat_style = "ranged".to_string();
        bot.world.stats.insert(1, (49, 0));
        let id = |bot: &Bot, name: &str| crate::kit::pack::obj_id(bot, name).expect("obj id");
        let worn_com = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME).expect("worn com");
        let dagger = id(&bot, "iron_dagger");
        let iron_arrow = id(&bot, "iron_arrow");
        bot.world.inv.insert(worn_com, HashMap::from([(3u8, (dagger, 1u32)), (13, (iron_arrow, 99))]));
        let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank com");
        let (bow, body, bronze) = (id(&bot, "shortbow"), id(&bot, "leather_armour"), id(&bot, "bronze_arrow"));
        bot.world.inv_closed.insert(bank_com, HashMap::from([(0u8, (bow, 2u32)), (1, (body, 1)), (2, (bronze, 1379)), (3, (dagger, 25))]));
        let plan = crate::kit::gear::gear_plan(&bot, &crate::kit::gear::owned_gear(&bot), crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
        let mut wants = super::banked_wants(&bot, &plan);
        wants.sort();
        let mut expected = vec![(bow, 1), (body, 1)];
        expected.sort();
        assert_eq!(wants, expected);
        bot.world.inv.insert(worn_com, HashMap::from([(3u8, (dagger, 1u32))]));
        assert!(super::banked_wants(&bot, &plan).contains(&(bronze, 1379)), "empty quiver: the arrows come too");
    }
}
