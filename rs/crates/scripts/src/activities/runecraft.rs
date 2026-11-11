//! Runecraft air..body (T): essence + talisman through the mysterious
//! ruins to the altar inside, exit portal out, banked loads through
//! `kit::cycle`. Essence comes from the mine via Aubury (P3.1, after Rune
//! Mysteries); this script spends banked essence + talisman and returns
//! runes for spells and sale (catalogue §5.8, P1 air + earth).
//! No tiaras in 274: altar entry is talisman-only (server `runecraft.rs2`).
//!
//! Process: poste ruines puis autel (`ruin`, `altar_rc`) · action lier
//! l'essence · interface directe (op1, pas de dialogue) · intrants essence
//! + talisman (non consommé) · produits runes (multiplicateur de niveau).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;
use super::runecraft_temple::{craft_at_altar, exit_temple};
use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::use_item_on_loc;
use crate::{ScriptError, ScriptResult};

pub const RUNECRAFT_STAT: u8 = 20;
pub const ESSENCE: &str = "blankrune";
/// (prefix, level, talisman, rune): six F2P altars, catalogue §5.8.
pub const RUNES: [(&str, i64, &str, &str); 6] = [
    ("air", 1, "air_talisman", "airrune"),
    ("mind", 2, "mind_talisman", "mindrune"),
    ("water", 5, "water_talisman", "waterrune"),
    ("earth", 9, "earth_talisman", "earthrune"),
    ("fire", 14, "fire_talisman", "firerune"),
    ("body", 20, "body_talisman", "bodyrune"),
];

/// Row for a rune, talisman or prefix name (`None` for members or junk).
pub fn rune_for(name: &str) -> Option<(&'static str, i64, &'static str, &'static str)> {
    RUNES.iter().find(|(prefix, _, talisman, rune)| prefix.eq_ignore_ascii_case(name) || talisman.eq_ignore_ascii_case(name) || rune.eq_ignore_ascii_case(name)).copied()
}

/// Overworld ruin tile of one prefix from the D57 stations (D63 F2P).
pub fn ruin_tile(prefix: &str) -> Option<(i64, i64, i64)> {
    let want = format!("{prefix}temple_ruined");
    knowledge::stations::stations_of("ruin").into_iter().find(|entry| entry.loc == want).map(|entry| (entry.x, entry.z, entry.level))
}

/// Top the pack up with the banked talisman (kept, never consumed) then
/// essence (nothing when the booth is far: the caller reports `blocked`).
fn fill_essence(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, essence: u16, talisman: u16) -> Result<(), ScriptError> {
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    if pack_total(bot, talisman) == 0 {
        bank_withdraw_items(bot, bank, &[talisman], 1)?;
    }
    let free = free_slots(bot) as u32;
    if free > 1 {
        bank_withdraw_items(bot, bank, &[essence], free - 1)?;
    }
    Ok(())
}

/// One full load: walk to the ruins, talisman in, craft at the altar,
/// portal out. Returns the essence the altar consumed. A sterile altar
/// never strands: the error reports the last game message and the portal
/// is still attempted before failing.
fn craft_load(
    bot: &mut Bot,
    essence: u16,
    talisman_name: &str,
    ruin: &str,
    altar: &str,
    portal: &str,
    prefix: &str,
    ruin_at: (i64, i64, i64),
) -> Result<u32, ScriptError> {
    bot.travel_to(lifescape_sdk::plan::Tile { x: ruin_at.0 as i32, z: ruin_at.1 as i32, level: ruin_at.2 as i32 })?;
    use_item_on_loc(bot, talisman_name, &[ruin])?;
    let made = match craft_at_altar(bot, essence, ESSENCE, altar) {
        Ok(made) => made,
        Err(error) => {
            let _ = exit_temple(bot, prefix, portal);
            return Err(error);
        }
    };
    exit_temple(bot, prefix, portal)?;
    Ok(made)
}

/// Craft banked loads in the open session: `runecraft [rune|talisman] [count]`
/// (air when omitted, 0 = forever). Missing essence or talisman blocks
/// with its name: the brain files the obtain task.
pub fn run_runecraft(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("runecraft", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let wanted = resource.as_deref().unwrap_or("air");
    let (prefix, need, talisman_name, rune_name) = rune_for(wanted).ok_or_else(|| ScriptError::Usage(format!("no F2P runecraft rune named {wanted:?} at {:?}", bot.position())))?;
    let level = learn_level(bot, RUNECRAFT_STAT, 1);
    if level < need {
        return Err(ScriptError::Usage(format!("{prefix} runes need runecraft {need}, bot is {level} at {:?}", bot.position())));
    }
    let essence = obj_id(bot, ESSENCE)?;
    let talisman = obj_id(bot, talisman_name)?;
    let _ = obj_id(bot, rune_name)?;
    let ruin = format!("{prefix}temple_ruined");
    let altar = format!("{prefix}_altar");
    let portal = format!("{prefix}temple_exit_portal");
    let ruin_at = ruin_tile(prefix).ok_or_else(|| ScriptError::Usage(format!("no {ruin} post in the dump at {:?}", bot.position())))?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_stock: Vec<u16> = vec![essence, talisman];
    keep_stock.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_stock, 0)?;
    fill_essence(bot, &bank, essence, talisman)?;
    if pack_total(bot, essence) == 0 {
        return Err(ScriptError::Blocked { missing: vec![ESSENCE.to_string()] });
    }
    if pack_total(bot, talisman) == 0 {
        return Err(ScriptError::Blocked { missing: vec![talisman_name.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, essence) == 0 || pack_total(step_bot, talisman) == 0 {
            bank_trip(step_bot, &bank, &keep_stock, 0)?;
            fill_essence(step_bot, &bank, essence, talisman)?;
            if pack_total(step_bot, essence) == 0 {
                return Err(ScriptError::Blocked { missing: vec![ESSENCE.to_string()] });
            }
            if pack_total(step_bot, talisman) == 0 {
                return Err(ScriptError::Blocked { missing: vec![talisman_name.to_string()] });
            }
        }
        let made = match craft_load(step_bot, essence, talisman_name, &ruin, &altar, &portal, prefix, ruin_at) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died runecrafting, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(made) => made,
        };
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &keep_stock, 0)?;
        step_bot.log(&format!("ok load {} bound {made} {rune_name} ({prefix}={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::{rune_for, ruin_tile, ESSENCE, RUNECRAFT_STAT, RUNES};
    use super::super::runecraft_temple::PORTALS;
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Six F2P rows match the extra runecraft recipes: level, essence in,
    /// rune out, ruin post (server `runecraft.dbrow` 50-75 tenths).
    #[test]
    fn table_matches_extra_recipes() {
        assert_eq!(RUNES.len(), 6);
        let extras = knowledge::extra_recipes::by_kind("runecraft");
        assert_eq!(extras.len(), 6);
        for (prefix, level, talisman, rune) in RUNES {
            let row = extras.iter().find(|entry| entry.source == format!("craft_{prefix}")).expect("craft row");
            assert_eq!(row.level, level, "{prefix} level");
            assert_eq!(row.station.as_deref(), Some("ruin"), "{prefix} post");
            assert!(row.inputs.iter().any(|(name, count)| name == ESSENCE && *count == 1), "{prefix} eats essence");
            assert!(row.inputs.iter().any(|(name, count)| name == talisman && *count == 1), "{prefix} needs its talisman");
            assert!(row.outputs.iter().any(|(name, count)| name == rune && *count == 1), "{prefix} pays its rune");
            assert_eq!(rune_for(rune), Some((prefix, level, talisman, rune)), "{rune} resolves");
            assert_eq!(rune_for(talisman), Some((prefix, level, talisman, rune)), "{talisman} resolves");
        }
        assert_eq!(rune_for("AIR"), Some(("air", 1, "air_talisman", "airrune")), "case-insensitive");
        assert!(rune_for("chaos_talisman").is_none(), "members never resolves");
        assert!(rune_for("lawrune").is_none(), "members never resolves");
        assert!(rune_for("coins").is_none(), "junk never resolves");
    }

    /// Essence, talismans and runes resolve through live tables, all F2P.
    #[test]
    fn ids_resolve_and_f2p() {
        let bot = offline_bot();
        obj_id(&bot, ESSENCE).expect("essence in tables");
        for (_, _, talisman, rune) in RUNES {
            obj_id(&bot, talisman).expect("talisman in tables");
            obj_id(&bot, rune).expect("rune in tables");
            for name in [ESSENCE, talisman, rune] {
                let found = knowledge::objs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} in dumps"));
                assert_eq!(found.members, Some(false), "{name} F2P");
            }
        }
    }

    /// Six ruins and six altars stand placed; every prefix resolves a ruin
    /// tile, and ruins/altars/exit portals exist in the loc dump.
    #[test]
    fn ruins_and_altars_stand_placed() {
        assert_eq!(knowledge::stations::stations_of("ruin").len(), 6, "ruins");
        assert_eq!(knowledge::stations::stations_of("altar_rc").len(), 6, "altars");
        for (prefix, _, _, _) in RUNES {
            assert!(ruin_tile(prefix).is_some(), "{prefix} ruin tile");
            assert!(!knowledge::locs_by_name(&format!("{prefix}temple_ruined")).is_empty(), "{prefix} ruins in locs");
            assert!(!knowledge::locs_by_name(&format!("{prefix}_altar")).is_empty(), "{prefix} altar in locs");
            assert!(!knowledge::locs_by_name(&format!("{prefix}temple_exit_portal")).is_empty(), "{prefix} portal in locs");
        }
        assert!(ruin_tile("cosmic").is_none(), "members never placed");
    }

    /// Exit portal rows cover exactly the six F2P altars, with distinct
    /// temple tiles (server land files l44_75..l39_75; the air pair is
    /// live-proven: altar 2843,4833, portal 2841,4828, exit 2983,3288).
    #[test]
    fn portals_cover_f2p_altars() {
        assert_eq!(PORTALS.len(), RUNES.len(), "one portal per F2P altar");
        for (prefix, id, x, z) in PORTALS {
            let row = rune_for(prefix).unwrap_or_else(|| panic!("{prefix} portal without a rune row"));
            assert_eq!(row.0, prefix, "{prefix} prefix");
            assert!((2465..=2470).contains(&id), "{prefix} portal id {id}");
            assert!((2500..2900).contains(&x) && (4820..4860).contains(&z), "{prefix} portal tile ({x},{z})");
        }
        assert!(PORTALS.contains(&("air", 2465, 2841, 4828)), "live-proven air portal");
        let mut tiles: Vec<(i32, i32)> = PORTALS.iter().map(|(_, _, x, z)| (*x, *z)).collect();
        tiles.sort();
        tiles.dedup();
        assert_eq!(tiles.len(), PORTALS.len(), "distinct portal tiles");
    }

    /// Level gate reads the live runecraft level (stat 20, quest-unlocked
    /// skill: 0 refused for air, 8 refused for earth, 9 passes).
    #[test]
    fn level_gate_reads_live_runecraft() {
        assert_eq!(lifescape_sdk::provenance::skill_id("runecraft"), Some(RUNECRAFT_STAT), "stat 20");
        let mut bot = offline_bot();
        bot.world.stats.insert(RUNECRAFT_STAT, (8, 0));
        let level = crate::kit::survival::learn_level(&bot, RUNECRAFT_STAT, 1);
        assert!(level < rune_for("earth").expect("earth").1, "8 refused for earth");
        assert!(level >= rune_for("air").expect("air").1, "8 passes for air");
    }

    /// Catalogue §2: no tiaras exist in the 274 dumps (talisman-only entry).
    #[test]
    fn no_tiaras_in_274() {
        assert!(knowledge::all_objs().iter().all(|entry| !entry.name.unwrap_or_default().contains("tiara")), "tiaras absent");
    }
}
