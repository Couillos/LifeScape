//! Mill grain (T): grain plus an empty pot through the hopper upstairs to
//! flour at the millbase. Lumbridge mill by default, the Cooks' Guild mill
//! when the bot holds its entry (32 Cooking, chef's hat). Two hoppers at
//! level 2, two millbases at level 0 (stations P2.1, catalogue §6, P1 for
//! Cook's Assistant, epoch P1).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;
use crate::kit::{bank::{bank_trip, bank_trip_resilient, bank_withdraw_items, nearest_bank}, cycle::{resolve_keep, run, StepControl}, pack::{free_slots, obj_id, pack_total}};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Catalogue §6 epoch and archetype (P1 mill harvest, T).
pub const EPOCH: &str = "P1";
pub const ARCH: &str = "T";
/// Hopper grain in, empty pot down, flour out.
pub const GRAIN: &str = "grain";
pub const POT_EMPTY: &str = "pot_empty";
pub const PRODUCT: &str = "pot_flour";
/// Cooks' Guild entry (services `GUILDS`, from the 274 scripts).
pub const GUILD_COOKING: i64 = 32;
pub const GUILD_HAT: &str = "chefs_hat";
pub const COOKING_STAT: u8 = 7;
/// The Varrock hopper serves the guild mill; Lumbridge needs no entry.
pub const GUILD_HOPPER: &str = "hopper_varrock";
/// One pack mills in a single upstairs/downstairs pass (bins hold 30).
pub const MILL_PASSES: u32 = 30;

/// Nearest hopper (the guild mill costs +10 000 tiles without 32 Cooking
/// and the hat, so Lumbridge serves instead — never a block on entry).
pub fn pick_hopper(bot: &Bot) -> Option<lifescape_gamedata::stations::Station> {
    // The brain's site (`--site`, the guild entry judged and the hat worn
    // by `--wear`): the hopper nearest it, never another mill.
    if let Some((x, z, _)) = bot.assigned_site {
        return lifescape_gamedata::stations::stations_of("hopper").into_iter().min_by_key(|hopper| (hopper.x - i64::from(x)).abs() + (hopper.z - i64::from(z)).abs());
    }
    let level = learn_level(bot, COOKING_STAT, 1);
    let hat = obj_id(bot, GUILD_HAT).ok().is_some_and(|id| pack_total(bot, id) > 0 || crate::kit::pack::is_worn(bot, id));
    lifescape_gamedata::stations::stations_of("hopper").into_iter().min_by_key(|hopper| (hopper.x - i64::from(bot.world.x)).abs() + (hopper.z - i64::from(bot.world.z)).abs() + if hopper.loc == GUILD_HOPPER && (level < GUILD_COOKING || !hat) { 10_000 } else { 0 })
}

/// Lever loc near the hopper (live stream only: `hopperlevers1-4` /
/// `hoppercontrol_inuse` in the 274 scripts).
pub fn lever_near(bot: &Bot, x: i32, z: i32, level: i32) -> Option<(i32, i32, u16)> {
    let mut found: Vec<(i64, (i32, i32, u16))> = Vec::new();
    for ((lx, lz, llevel), slot) in &bot.world.locs {
        if let Some((loc, _, _)) = slot.as_ref().filter(|_| *llevel == level) {
            let name = lifescape_gamedata::loc_by_id(usize::from(*loc)).and_then(|entry| entry.name).unwrap_or_default().to_lowercase();
            if (name.contains("hopperlever") || name.contains("hoppercontrol")) && (lx - x).abs() + (lz - z).abs() <= 8 {
                found.push((i64::from((lx - x).abs() + (lz - z).abs()), (*lx, *lz, *loc)));
            }
        }
    }
    found.into_iter().filter(|(dist, _)| *dist <= 8).min_by_key(|(dist, _)| *dist).map(|(_, tile)| tile)
}

/// Mill one pack: upstairs the grain slides down the chute, downstairs the
/// pots fill from the bin. Returns (milled, taken).
fn mill_pack(bot: &mut Bot, hopper: &lifescape_gamedata::stations::Station, grain: u16, pot: u16, flour: u16) -> Result<(u32, u32), ScriptError> {
    let (hx, hz, hl) = (hopper.x as i32, hopper.z as i32, hopper.level as i32);
    bot.travel_to(lifescape_sdk::plan::Tile { x: hx, z: hz, level: hl })?;
    let (hopper_id, _, _) = bot.world.loc_at(hx, hz, hl).ok_or_else(|| ScriptError::Failed(format!("no hopper loc at ({hx},{hz})")))?;
    let Some((lever_x, lever_z, lever_id)) = lever_near(bot, hx, hz, hl) else {
        return Err(ScriptError::Failed(format!("no hopper lever near ({hx},{hz}) at {:?}", bot.position())));
    };
    let grain_before = pack_total(bot, grain);
    let mut passes = 0;
    while pack_total(bot, grain) > 0 && passes < MILL_PASSES {
        passes += 1;
        let (use_com, use_slot) = crate::quests::common::inv_slot(bot, grain)?;
        bot.op_loc_u(hx, hz, hopper_id, grain, u16::from(use_slot), use_com)?;
        bot.pump(3)?;
        bot.op_loc(1, lever_x, lever_z, lever_id)?;
        bot.pump(5)?;
    }
    let milled = grain_before.saturating_sub(pack_total(bot, grain));
    // Nearest ground-level mill-kind tile: Lumbridge sits under its hopper,
    // the Varrock millbase stands one tile west of its hopper.
    let (bx, bz) = lifescape_gamedata::stations::stations_of("mill").iter().filter(|mill| mill.level == 0)
        .min_by_key(|mill| (mill.x - hopper.x).abs() + (mill.z - hopper.z).abs())
        .map(|mill| (mill.x as i32, mill.z as i32)).ok_or_else(|| ScriptError::Failed(format!("no millbase near ({hx},{hz})")))?;
    bot.travel_to(lifescape_sdk::plan::Tile { x: bx, z: bz, level: 0 })?;
    let Some((mill_id, _, _)) = bot.world.loc_at(bx, bz, 0) else {
        return Ok((milled, 0));
    };
    let flour_before = pack_total(bot, flour);
    let mut takes = 0;
    while pack_total(bot, pot) > 0 && takes < MILL_PASSES {
        takes += 1;
        let had = pack_total(bot, flour);
        bot.op_loc(1, bx, bz, mill_id)?;
        bot.pump(5)?;
        if pack_total(bot, flour) <= had {
            break;
        }
    }
    Ok((milled, pack_total(bot, flour).saturating_sub(flour_before)))
}

/// Grain and pots in the pack, topped up from the bank (cook pattern).
/// Empty on both sides blocks with the missing input: the brain files the
/// obtain task (grain from `picking`, pots from pottery).
fn ensure_inputs(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, grain: u16, pot: u16) -> Result<bool, ScriptError> {
    if pack_total(bot, grain) == 0 || pack_total(bot, pot) == 0 {
        bank_trip(bot, bank, &[], 0)?;
        let free = free_slots(bot) as u32;
        bank_withdraw_items(bot, bank, &[pot], (free + 1) / 2)?;
        bank_withdraw_items(bot, bank, &[grain], free.saturating_sub(pack_total(bot, pot).min(free)))?;
    }
    Ok(pack_total(bot, grain) > 0 && pack_total(bot, pot) > 0)
}

/// Mill banked loads in the open session: `mill_grain [count]`.
pub fn run_mill_grain(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("mill_grain", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let grain = obj_id(bot, GRAIN)?;
    let pot = obj_id(bot, POT_EMPTY)?;
    let flour = obj_id(bot, PRODUCT)?;
    let mut bank = nearest_bank(bot)?;
    if !ensure_inputs(bot, &bank, grain, pot)? {
        let mut missing = Vec::new();
        if pack_total(bot, grain) == 0 {
            missing.push(GRAIN.to_string());
        }
        if pack_total(bot, pot) == 0 {
            missing.push(POT_EMPTY.to_string());
        }
        return Err(ScriptError::Blocked { missing });
    }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if !ensure_inputs(step_bot, &bank, grain, pot)? {
            return Err(ScriptError::Blocked { missing: vec![GRAIN.to_string(), POT_EMPTY.to_string()] });
        }
        let hopper = pick_hopper(step_bot).ok_or_else(|| ScriptError::Failed(format!("no usable mill hopper near {:?}", step_bot.position())))?;
        let (milled, taken) = mill_pack(step_bot, &hopper, grain, pot, flour)?;
        if milled + taken == 0 { return Err(ScriptError::Usage(format!("mill gave nothing at {:?}", step_bot.position()))); }
        step_bot.log(&format!("ok milled {milled} grain, took {taken} flour"));
        bank_trip_resilient(step_bot, &mut bank, &task_keep, 0, &[flour])?;
        step_bot.log(&format!("ok load {} milled", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

#[cfg(test)]
mod tests {
    use super::{lever_near, pick_hopper, ARCH, COOKING_STAT, EPOCH, GUILD_COOKING, GUILD_HAT, GUILD_HOPPER, POT_EMPTY, PRODUCT};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Declaration: epoch P1, archetype T, grain in and flour out resolve
    /// F2P, the guild entry matches the services table.
    #[test]
    fn declaration_matches_dump_and_services() {
        assert_eq!((EPOCH, ARCH), ("P1", "T"));
        let bot = offline_bot();
        for name in [super::GRAIN, POT_EMPTY, PRODUCT, GUILD_HAT] {
            obj_id(&bot, name).expect("mill name in tables");
            let row = knowledge::objs_by_name(name).into_iter().next().expect("dump row");
            assert_eq!(row.members, Some(false), "{name} is F2P, got {:?}", row.members);
        }
        assert_eq!(COOKING_STAT, 7, "cooking stat");
        assert_eq!((GUILD_COOKING, GUILD_HOPPER), (32, "hopper_varrock"));
        let guilds: Vec<(&str, i64, &str)> = knowledge::services::GUILDS.to_vec();
        assert!(guilds.contains(&("cooks_guild", 32, "chefs_hat")), "guild entry, got {guilds:?}");
    }

    /// Two hoppers stand at level 2 (Lumbridge and the guild mill), each
    /// above its ground-level millbase (Lumbridge under its hopper, Varrock
    /// one tile west of its hopper).
    #[test]
    fn hoppers_stand_over_their_mills() {
        let hoppers = knowledge::stations::stations_of("hopper");
        assert_eq!(hoppers.len(), 2, "hoppers, got {hoppers:?}");
        for hopper in &hoppers {
            assert_eq!(hopper.level, 2, "hopper upstairs, got {hopper:?}");
            let mills = knowledge::stations::stations_of("mill");
            let base = mills.iter().filter(|mill| mill.level == 0)
                .min_by_key(|mill| (mill.x - hopper.x).abs() + (mill.z - hopper.z).abs()).expect("a millbase");
            assert!((base.x - hopper.x).abs() + (base.z - hopper.z).abs() <= 1, "millbase by ({},{}), got {base:?}", hopper.x, hopper.z);
        }
        assert!(hoppers.iter().any(|hopper| hopper.loc == "hopper_lumbridge" && (hopper.x, hopper.z) == (3166, 3307)));
        assert!(hoppers.iter().any(|hopper| hopper.loc == GUILD_HOPPER && (hopper.x, hopper.z) == (3140, 3450)));
        let bases: Vec<(i64, i64)> = knowledge::stations::stations_of("mill").iter().filter(|mill| mill.level == 0).map(|mill| (mill.x, mill.z)).collect();
        assert!(bases.contains(&(3166, 3307)) && bases.contains(&(3139, 3450)), "millbases, got {bases:?}");
    }

    /// Nearest usable hopper wins: Lumbridge from Lumbridge with no entry,
    /// and a hatless level 1 skips the guild mill for Lumbridge.
    #[test]
    fn pick_prefers_usable_hoppers() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 3166;
        bot.world.z = 3307;
        let near = pick_hopper(&bot).expect("a hopper picks");
        assert_eq!(near.loc, "hopper_lumbridge", "Lumbridge wins from Lumbridge, got {near:?}");
        bot.world.stats.insert(COOKING_STAT, (1, 0));
        bot.world.x = 3140;
        bot.world.z = 3450;
        let usable = pick_hopper(&bot).expect("a usable hopper picks");
        assert_eq!(usable.loc, "hopper_lumbridge", "hatless level 1 skips the guild mill, got {usable:?}");
    }

    /// Lever scan: a live `hopperlevers` loc near the hopper resolves, far
    /// tiles and other levels never do.
    #[test]
    fn lever_resolves_live_only() {
        let mut bot = offline_bot();
        bot.world.level = 2;
        let lever = (0u16..).find(|id| knowledge::loc_by_id(usize::from(*id)).and_then(|entry| entry.name).unwrap_or_default().to_lowercase().contains("hopperlever")).expect("a lever loc exists");
        bot.world.locs.insert((3167, 3307, 2), Some((lever, 0, 0)));
        assert_eq!(lever_near(&mut bot, 3166, 3307, 2), Some((3167, 3307, lever)), "lever resolves");
        assert!(lever_near(&mut bot, 3166, 3307, 0).is_none(), "other levels never match");
        assert!(lever_near(&mut bot, 3000, 3000, 2).is_none(), "far tiles never match");
    }
}
