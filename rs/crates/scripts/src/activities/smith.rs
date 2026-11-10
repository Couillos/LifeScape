//! Smith: bars + hammer on an anvil (catalogue §5.7, P1). Bar on an
//! anvil opens the smithing columns; one Make press per item on the
//! product cell, then wait out the bar consumption. The 274 column
//! invs are uniform across bars (dagger..2h / axe..battleaxe /
//! chainbody..platebody / med_helm..kiteshield; steel nails alone on
//! column5), so the cell map is a shared suffix table plus the bar
//! prefix, never a per-product hardcode. P1 always forges the highest
//! platebody; Doric anvils need Doric's Quest (server gate, loud).
//!
//! Process: poste enclume · action forger · interface colonnes ·
//! intrants barre + marteau · produits pièce forgée.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::use_item_on_loc;
use crate::{ScriptError, ScriptResult};

pub const SMITHING_STAT: u8 = 13;
pub const HAMMER: &str = "hammer";
pub const ANVILS: [&str; 2] = ["anvil", "doric_anvil"];
/// Bar debugname and the product prefix the 274 column invs use
/// (adamantite and runite bars shorten in product names).
pub const BAR_PREFIX: [(&str, &str); 6] = [("bronze_bar", "bronze"), ("iron_bar", "iron"), ("steel_bar", "steel"), ("mithril_bar", "mithril"), ("adamantite_bar", "adamant"), ("runite_bar", "rune")];
/// Column cells in interface order (slot = position): the four F2P
/// columns shared by every bar (steel nails sit on column5 instead).
pub const COLUMNS: [&[&str]; 4] = [&["dagger", "sword", "scimitar", "longsword", "2h_sword"], &["axe", "mace", "warhammer", "battleaxe"], &["chainbody", "platelegs", "plateskirt", "platebody"], &["med_helm", "full_helm", "sq_shield", "kiteshield"]];
/// Highest platebody at each floor (catalogue §5.7 P1): product, level.
pub const PLATEBODIES: [(&str, i64); 6] = [("bronze_platebody", 18), ("iron_platebody", 33), ("steel_platebody", 48), ("mithril_platebody", 68), ("adamant_platebody", 88), ("rune_platebody", 99)];
const ITEM_TICKS: u64 = 40;

/// F2P smith recipe making `product` (members excluded).
pub fn smith_recipe(product: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::making(product, Some("smith")).into_iter().find(|entry| entry.members == Some(false))
}

/// Default product at `level`: the highest platebody unlocked, else a
/// bronze dagger (P1 « toujours le corps de plate le plus haut »).
pub fn product_for_level(level: i64) -> &'static str {
    PLATEBODIES.iter().rev().find(|(_, need)| level >= *need).map(|(name, _)| *name).unwrap_or("bronze_dagger")
}

/// Smithing column cell making `product`: 1-based column, 0-based slot
/// in the 274 column invs (`None` for members cells and junk: never a
/// blind press, the run fails loud instead).
pub fn smith_spot(product: &str) -> Option<(u8, u8)> {
    let lower = product.to_lowercase();
    if lower == "nails" { return Some((5, 3)); }
    // 274 keeps the Jagex column typo (column2 slot2 of adamant).
    if lower == "adamnt_warhammer" { return Some((2, 2)); }
    let (head, suffix) = lower.split_once('_')?;
    if !BAR_PREFIX.iter().any(|(_, prefix)| *prefix == head) { return None; }
    COLUMNS.iter().enumerate().find_map(|(column, cells)| cells.iter().position(|cell| *cell == suffix).map(|slot| (column as u8 + 1, slot as u8)))
}

/// Smith banked loads in the open session: `smith [product] [count]`
/// (highest platebody by level when omitted, 0 = forever).
pub fn run_smith(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("smith", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, SMITHING_STAT, 1);
    let product_name = resource.as_deref().unwrap_or_else(|| product_for_level(level));
    let recipe = smith_recipe(product_name).ok_or_else(|| ScriptError::Usage(format!("no F2P smith recipe makes {product_name:?} at {:?}", bot.position())))?;
    if level < recipe.level {
        return Err(ScriptError::Usage(format!("smithing {product_name} needs level {} at {:?}", recipe.level, bot.position())));
    }
    let (bar_name, bars_each) = recipe.inputs.first().ok_or_else(|| ScriptError::Usage(format!("smith {product_name} has no bar input at {:?}", bot.position())))?;
    let bars_each = *bars_each as u32;
    let (column, slot) = smith_spot(product_name).ok_or_else(|| ScriptError::Usage(format!("no F2P smithing column cell makes {product_name:?} at {:?}", bot.position())))?;
    let bar = obj_id(bot, bar_name)?;
    let hammer = obj_id(bot, HAMMER)?;
    let product = obj_id(bot, product_name)?;
    let mut bank = nearest_bank(bot)?;
    let post = knowledge::stations::nearest_station("anvil", bank.x, bank.z, bank.level).ok_or_else(|| ScriptError::Usage(format!("no anvil post near bank at {:?}", bot.position())))?;
    bot.log(&format!("anvil post at ({},{})", post.x, post.z));
    let mut keep_smith: Vec<u16> = vec![bar, hammer];
    keep_smith.extend(task_keep.iter().copied());
    restock(bot, &bank, &keep_smith, bar, hammer, bars_each, bar_name)?;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, bar) < bars_each || pack_total(step_bot, hammer) == 0 {
            restock(step_bot, &bank, &keep_smith, bar, hammer, bars_each, bar_name)?;
        }
        let made_before = pack_total(step_bot, product);
        while pack_total(step_bot, bar) >= bars_each && free_slots(step_bot) > 0 {
            match smith_one(step_bot, bar_name, product, column, slot) {
                Err(error) if is_death_abort(&error) => {
                    step_bot.log("died smithing, restocking to retry");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
                Ok(()) => {}
            }
        }
        let made = pack_total(step_bot, product).saturating_sub(made_before);
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} smithed {made} {product_name} (smithing={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

/// Restock bars with the hammer kept, never banked (loud `blocked`
/// when the bank holds none of either, never an empty walk).
fn restock(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, keep: &[u16], bar: u16, hammer: u16, bars_each: u32, bar_name: &str) -> Result<(), ScriptError> {
    bank_trip(bot, bank, keep, 0)?;
    if pack_total(bot, hammer) == 0 {
        return Err(ScriptError::Blocked { missing: vec![HAMMER.to_string()] });
    }
    let room = (free_slots(bot) as u32).saturating_sub(2);
    bank_withdraw_items(bot, bank, &[bar], room * bars_each)?;
    if pack_total(bot, bar) < bars_each {
        return Err(ScriptError::Blocked { missing: vec![bar_name.to_string()] });
    }
    Ok(())
}

/// Forge one item: bar on the nearest anvil, Make press on the product
/// cell, wait out the bar consumption. A missing modal is loud (wrong
/// post, or a Doric anvil before Doric's Quest).
fn smith_one(bot: &mut Bot, bar_name: &str, product: u16, column: u8, slot: u8) -> Result<(), ScriptError> {
    let had = pack_total(bot, product);
    use_item_on_loc(bot, bar_name, &ANVILS)?;
    let target = bot.world.ticks + ITEM_TICKS;
    while bot.world.ticks < target && bot.world.modal.is_none() {
        bot.pump(2)?;
    }
    if bot.world.modal.is_none() {
        return Err(ScriptError::Usage(format!("smithing interface never opened at {:?} (Doric anvils need Doric's Quest)", bot.position())));
    }
    let column_com = (bot.tables.com_id)(&format!("smithing:column{column}")).ok_or_else(|| ScriptError::Usage(format!("unknown smithing column{column} at {:?}", bot.position())))?;
    bot.inv_button(1, product, slot, column_com)?;
    let target = bot.world.ticks + ITEM_TICKS;
    while bot.world.ticks < target && pack_total(bot, product) <= had {
        bot.pump(2)?;
    }
    if bot.world.modal.is_some() {
        bot.close_modal()?;
    }
    if pack_total(bot, product) <= had {
        return Err(ScriptError::Usage(format!("nothing smithed at {:?}", bot.position())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{product_for_level, smith_recipe, smith_spot, ANVILS, BAR_PREFIX, COLUMNS, HAMMER, PLATEBODIES};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.7 forge comes from the dump (103 F2P rows, not 102:
    /// the steel nails row is F2P too): six bars, platebody levels and
    /// bar counts, nails making 2 from 1 bar.
    #[test]
    fn forge_ladder_comes_from_gamedata() {
        let all = lifescape_gamedata::recipes::all_recipes(Some("smith"));
        let f2p: Vec<_> = all.iter().filter(|entry| entry.members == Some(false)).collect();
        assert_eq!(f2p.len(), 103, "F2P smith rows (catalogue says 102, nails extra)");
        assert_eq!(PLATEBODIES.map(|(_, level)| level), [18, 33, 48, 68, 88, 99]);
        for (product, level) in PLATEBODIES {
            let recipe = smith_recipe(product).unwrap_or_else(|| panic!("{product} recipe resolves"));
            assert_eq!(recipe.kind, "smith");
            assert_eq!(recipe.level, level, "{product} level");
            assert_eq!(recipe.inputs.len(), 1, "{product} eats one bar");
            assert_eq!(recipe.inputs[0].1, 5, "{product} eats 5 bars");
        }
        let nails = smith_recipe("nails").expect("steel nails resolve");
        assert_eq!((nails.level, nails.inputs.clone(), nails.outputs.clone()), (34, vec![("steel_bar".to_string(), 1)], vec![("nails".to_string(), 2)]));
        for (bar, _) in BAR_PREFIX {
            assert_eq!(lifescape_gamedata::recipes::using(bar, Some("smith")).iter().filter(|entry| entry.members == Some(false)).count(), if bar == "steel_bar" { 18 } else { 17 }, "{bar} row count");
        }
        assert!(smith_recipe("bronze_dart_tip").is_none(), "members dart tips excluded");
        assert!(smith_recipe("BRONZE_DAGGER").is_some(), "case-insensitive");
    }

    /// Default pick: highest platebody at the level, dagger below 18.
    #[test]
    fn default_pick_is_highest_platebody() {
        assert_eq!(product_for_level(1), "bronze_dagger");
        assert_eq!(product_for_level(17), "bronze_dagger");
        assert_eq!(product_for_level(18), "bronze_platebody");
        assert_eq!(product_for_level(33), "iron_platebody");
        assert_eq!(product_for_level(99), "rune_platebody");
    }

    /// Column cells come from the uniform 274 layout (slot = position):
    /// platebodies on column3, daggers on column1, steel nails alone on
    /// column5; members cells and junk never map (no blind press).
    #[test]
    fn column_cells_come_from_the_uniform_layout() {
        for (product, column, slot) in [("bronze_dagger", 1, 0), ("bronze_platebody", 3, 3), ("rune_platebody", 3, 3), ("adamant_platebody", 3, 3), ("adamnt_warhammer", 2, 2), ("iron_axe", 2, 0), ("steel_kiteshield", 4, 3), ("mithril_2h_sword", 1, 4), ("nails", 5, 3)] {
            assert_eq!(smith_spot(product), Some((column, slot)), "{product} cell");
            assert_eq!(smith_spot(product.to_uppercase().as_str()), Some((column, slot)), "{product} case-insensitive");
        }
        assert!(smith_spot("bronze_dart_tip").is_none(), "members column5 never maps");
        assert!(smith_spot("bronze_claws").is_none(), "members claws never map");
        assert!(smith_spot("coins").is_none(), "junk never maps");
        assert!(smith_spot("steel_bar").is_none(), "bars never map");
    }

    /// Posts, hammer and column coms resolve: anvil locs stand in the
    /// anvil dump (17 + 2 Doric), columns through live tables.
    #[test]
    fn anvil_posts_hammer_and_columns_resolve() {
        let anvils = lifescape_gamedata::stations::stations_of("anvil");
        assert_eq!(anvils.len(), 19, "anvil tiles (17 + 2 doric)");
        for name in ANVILS {
            assert!(anvils.iter().any(|entry| entry.loc == *name), "anvil loc {name} placed");
        }
        let post = lifescape_gamedata::stations::nearest_station("anvil", 2960, 3380, 0).expect("a post near Rimmington");
        assert_eq!(post.kind, "anvil");
        let bot = offline_bot();
        assert!(obj_id(&bot, HAMMER).is_ok(), "hammer resolves");
        for (bar, prefix) in BAR_PREFIX {
            assert!(obj_id(&bot, bar).is_ok(), "{bar} resolves");
            for cells in COLUMNS {
                for suffix in cells.iter() {
                    let name = if prefix == "adamant" && *suffix == "warhammer" { "adamnt_warhammer".to_string() } else { format!("{prefix}_{suffix}") };
                    assert!(obj_id(&bot, &name).is_ok(), "{name} resolves");
                }
            }
        }
        for product in ["nails", "bronze_dagger", "rune_platebody"] {
            let recipe = smith_recipe(product).expect("recipe");
            for (name, _) in &recipe.inputs {
                assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
            }
            assert!(obj_id(&bot, product).is_ok(), "{product} resolves");
        }
        for column in 1u8..=5 {
            let name = format!("smithing:column{column}");
            assert!((bot.tables.com_id)(&name).is_some(), "{name} resolves");
        }
    }
}
