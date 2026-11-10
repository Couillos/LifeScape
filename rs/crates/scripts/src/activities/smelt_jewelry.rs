//! Smelted jewellery: gold bars (plain or gem-set) and silver holy
//! symbols at a furnace (catalogue §5.6, P1+P2). Gold opens the
//! `crafting_jewelry` grid: the slot mapping is the server script
//! (slot 0 plain, 1 sapphire, 2 emerald, 3 ruby, 4 diamond; dragonstone
//! never matches, members), read live and pressed with `inv_button`
//! through the shared grid adapter. Silver symbols take the
//! `skill_multi2` make-x grid through the produce brick. Moulds are kept.
//!
//! Process: poste four (4 sites, 8 cases) · action fondre · interface
//! grilles · intrants barre + gemme + moule · produits bijoux.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi, grid_button};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::{find_loc, inv_slot};
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const GOLD_BAR: &str = "gold_bar";
pub const SILVER_BAR: &str = "silver_bar";
pub const SILVER_PRODUCT: &str = "nostringstar";
pub const DEFAULT_PRODUCT: &str = "gold_ring";
/// Furnace loc debugnames (exact: the dump pairs furnace1+furnace2 tiles
/// per site, 4 sites; `produce` matches exactly, never contains).
pub const FURNACES: [&str; 2] = ["furnace1", "furnace2"];

const SILVER: Produce<'static> = Produce { verb: "casting", site: ProduceSite::Loc(&FURNACES), ui: ProduceUi::Grid { button: "skill_multi2:makex_b" } };

/// F2P row for `product`: interface, slot, gem ("" when plain),
/// mould, level. Gold slots follow the server script (0 plain,
/// 1 sapphire, 2 emerald, 3 ruby, 4 diamond); dragonstone never matches
/// (members). Silver takes the make-x grid. Levels come from the dump.
pub fn jewel_row(product: &str) -> Option<(&'static str, u8, &'static str, &'static str, i64)> {
    if product.eq_ignore_ascii_case(SILVER_PRODUCT) {
        return Some(("skill_multi2", 1, "", "holy_symbol_mould", 16));
    }
    let lower = product.to_lowercase();
    let (kind, stem) = if let Some(stem) = lower.strip_suffix("_ring") { ("ring", stem) } else if let Some(stem) = lower.strip_suffix("_necklace") { ("necklace", stem) } else if let Some(stem) = lower.strip_suffix("_amulet") { ("amulet", stem) } else { return None };
    let metal = stem.strip_prefix("unstrung_").unwrap_or(stem);
    let (slot, gem) = match metal { "gold" => (0, ""), "sapphire" => (1, "sapphire"), "emerald" => (2, "emerald"), "ruby" => (3, "ruby"), "diamond" => (4, "diamond"), _ => return None };
    let (inv, mould) = match kind { "ring" => ("crafting_jewelry:rings_inv", "ring_mould"), "necklace" => ("crafting_jewelry:necklaces_inv", "necklace_mould"), _ => ("crafting_jewelry:amulets_inv", "amulet_mould") };
    let recipe = lifescape_gamedata::extra_recipes::all_extra().into_iter().find(|entry| entry.kind == "jewellery" && entry.outputs.iter().any(|(name, _)| name.eq_ignore_ascii_case(product)))?;
    Some((inv, slot, gem, mould, recipe.level))
}

/// Smelt banked loads in the open session: `smelt_jewelry [product]
/// [count]` (gold ring when omitted, 0 = forever).
pub fn run_smelt_jewelry(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("smelt_jewelry", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    let product_name = resource.as_deref().unwrap_or(DEFAULT_PRODUCT);
    let silver = product_name.eq_ignore_ascii_case(SILVER_PRODUCT);
    let (inv_name, slot, gem_name, mould_name, need) = jewel_row(product_name).ok_or_else(|| ScriptError::Usage(format!("{product_name:?} is no F2P jewel at {:?}", bot.position())))?;
    if level < need { return Err(ScriptError::Usage(format!("casting {product_name} needs level {need} at {:?}", bot.position()))); }
    let bar = obj_id(bot, if silver { SILVER_BAR } else { GOLD_BAR })?;
    let mould = obj_id(bot, mould_name)?;
    let gem = if gem_name.is_empty() { None } else { Some(obj_id(bot, gem_name)?) };
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_cast: Vec<u16> = vec![mould];
    keep_cast.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_cast, 0)?;
    fill_cast(bot, &bank, bar, gem, mould)?;
    if pack_total(bot, bar) == 0 { return Err(ScriptError::Blocked { missing: vec![if silver { SILVER_BAR } else { GOLD_BAR }.to_string()] }); }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, bar) == 0 || gem.is_some_and(|id| pack_total(step_bot, id) == 0) {
            bank_trip(step_bot, &bank, &keep_cast, 0)?;
            fill_cast(step_bot, &bank, bar, gem, mould)?;
            if pack_total(step_bot, bar) == 0 { return Err(ScriptError::Blocked { missing: vec![if silver { SILVER_BAR } else { GOLD_BAR }.to_string()] }); }
        }
        if silver {
            let job = ProduceJob { spec: &SILVER, input: bar, input_name: SILVER_BAR, want: 0, products: vec![obj_id(step_bot, SILVER_PRODUCT)?], near: (bank.x as i32, bank.z as i32) };
            if cast_outcome(produce::run_job(step_bot, &job), step_bot)?.is_none() { return Ok(StepControl::Continue); }
        } else {
            let (loc_x, loc_z, level, loc) = find_loc(step_bot, &["furnace"], 150)?;
            step_bot.approach(loc_x, loc_z, level, 2, 3000)?;
            while pack_total(step_bot, bar) > 0 && gem.is_none_or(|id| pack_total(step_bot, id) > 0) {
                if cast_outcome(jewel_one(step_bot, loc_x, loc_z, loc, bar, inv_name, slot), step_bot)?.is_none() { return Ok(StepControl::Continue); }
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} cast {product_name} (crafting={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

/// Death restocks and retries the load (`Ok(None)` means retried).
fn cast_outcome<Done>(result: Result<Done, ScriptError>, bot: &mut Bot) -> Result<Option<Done>, ScriptError> {
    match result {
        Err(error) if is_death_abort(&error) => { bot.log("died casting, restocking to retry"); Ok(None) }
        Err(error) => Err(error),
        Ok(done) => Ok(Some(done)),
    }
}

/// One press of gold jewellery: bar on the furnace opens the grid, the
/// live slot proves the mould and gem are there, Make 10/5/1 by
/// remainder. A blind press never happens (empty slot fails loud).
fn jewel_one(bot: &mut Bot, furnace_x: i32, furnace_z: i32, furnace: u16, bar: u16, inv_name: &str, slot: u8) -> Result<(), ScriptError> {
    let had = pack_total(bot, bar);
    if bot.world.modal.is_none() {
        let (use_com, use_slot) = inv_slot(bot, bar)?;
        bot.op_loc_u(furnace_x, furnace_z, furnace, bar, u16::from(use_slot), use_com)?;
        let target = bot.world.ticks + 30;
        while bot.world.modal.is_none() && bot.world.ticks < target { bot.pump(2)?; }
        if bot.world.modal.is_none() {
            return Err(ScriptError::Usage(format!("jewellery grid never opened at {:?}", bot.position())));
        }
    }
    let com = grid_button(bot, inv_name)?;
    let held = bot.world.inv.get(&com).and_then(|slots| slots.get(&slot)).map(|(held, _)| *held).ok_or_else(|| ScriptError::Usage(format!("jewellery slot {slot} empty: need the mould and gem at {:?}", bot.position())))?;
    let (op, count) = if had >= 10 { (3u8, 10u64) } else if had >= 5 { (2, 5) } else { (1, 1) };
    bot.inv_button(op, held, slot, com)?;
    let target = bot.world.ticks + 20 + 12 * count;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if pack_total(bot, bar) < had { return Ok(()); }
    }
    Err(ScriptError::Usage(format!("jewellery never cast at {:?}", bot.position())))
}

/// Top the pack up: mould first (kept across loads), then bars and gems
/// half and half (nothing when the booth is far).
fn fill_cast(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, bar: u16, gem: Option<u16>, mould: u16) -> Result<(), ScriptError> {
    let far = (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12;
    if far { return Ok(()); }
    if pack_total(bot, mould) == 0 { bank_withdraw_items(bot, bank, &[mould], 1)?; }
    if free_slots(bot) > 1 {
        if let Some(gem) = gem {
            bank_withdraw_items(bot, bank, &[gem], free_slots(bot) as u32 / 2)?;
        }
        bank_withdraw_items(bot, bank, &[bar], free_slots(bot) as u32)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{jewel_row, DEFAULT_PRODUCT, GOLD_BAR, SILVER_BAR, SILVER_PRODUCT};
    use crate::kit::pack::obj_id;
    use crate::kit::produce::grid_button;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Every named jewel resolves to the dump row (kind, level, mould,
    /// gem) with the server slot; dragonstone never matches (members).
    #[test]
    fn gold_rows_match_the_dump() {
        for product in ["gold_ring", "gold_necklace", "unstrung_gold_amulet", "sapphire_ring", "sapphire_necklace", "unstrung_sapphire_amulet", "emerald_ring", "emerald_necklace", "unstrung_emerald_amulet", "ruby_ring", "ruby_necklace", "unstrung_ruby_amulet", "diamond_ring", "diamond_necklace", "unstrung_diamond_amulet"] {
            let (inv, slot, gem, mould, level) = jewel_row(product).unwrap_or_else(|| panic!("{product} rows"));
            let recipe = lifescape_gamedata::extra_recipes::all_extra().into_iter().find(|entry| entry.kind == "jewellery" && entry.outputs.iter().any(|(name, _)| name == product)).unwrap_or_else(|| panic!("{product} jewellery row"));
            assert_eq!(level, recipe.level, "{product} level from the dump");
            assert!(recipe.inputs.iter().any(|(name, _)| name == mould), "{product} eats {mould}");
            if gem.is_empty() {
                assert_eq!(slot, 0, "{product} plain sits in slot 0");
                assert!(inv.contains("rings") || inv.contains("necklaces") || inv.contains("amulets"), "{inv}");
            } else {
                assert!((1..=4).contains(&slot), "{product} gem slot F2P");
                assert!(recipe.inputs.iter().any(|(name, _)| name == gem), "{product} eats {gem}");
            }
        }
        assert_eq!(jewel_row("GOLD_RING"), jewel_row("gold_ring"), "case-insensitive");
        assert_eq!(jewel_row(SILVER_PRODUCT), Some(("skill_multi2", 1, "", "holy_symbol_mould", 16)), "silver takes the make-x grid");
        assert!(jewel_row("dragonstone_ring").is_none(), "members never cast");
        assert!(jewel_row("gold_bar").is_none(), "bars never cast");
        assert!(jewel_row(DEFAULT_PRODUCT).is_some(), "default casts");
    }

    /// Four furnace sites (eight tiles), both grid coms and every
    /// bar, mould, gem and product resolve through live tables.
    #[test]
    fn furnaces_grids_and_names_resolve() {
        assert_eq!(lifescape_gamedata::stations::stations_of("furnace").len(), 8, "furnace tiles");
        let bot = offline_bot();
        assert!(grid_button(&bot, "crafting_jewelry:rings_inv").is_ok(), "gold grid resolves");
        assert!(grid_button(&bot, "crafting_jewelry:amulets_inv").is_ok(), "amulet grid resolves");
        assert!(grid_button(&bot, "skill_multi2:makex_b").is_ok(), "silver make-x resolves");
        assert!(obj_id(&bot, GOLD_BAR).is_ok(), "gold resolves");
        assert!(obj_id(&bot, SILVER_BAR).is_ok(), "silver resolves");
        assert!(obj_id(&bot, SILVER_PRODUCT).is_ok(), "holy symbol resolves");
        for product in ["gold_ring", "sapphire_ring", "emerald_necklace", "unstrung_ruby_amulet", "diamond_necklace"] {
            let (_, _, gem, mould, _) = jewel_row(product).expect("row");
            assert!(obj_id(&bot, product).is_ok(), "{product} resolves");
            assert!(obj_id(&bot, mould).is_ok(), "{mould} resolves");
            if !gem.is_empty() {
                assert!(obj_id(&bot, gem).is_ok(), "{gem} resolves");
            }
        }
    }
}
