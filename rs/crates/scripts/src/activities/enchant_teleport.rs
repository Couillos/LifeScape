//! Enchant and travel magic: amulet enchantment (7/27/49/57), the three
//! F2P teleports (25/31/37) and bones to bananas (15), all catalogue
//! §5.10. Levels and runes come from the spell dump and are
//! unit-tested; runes and targets are stocked from the pack and the
//! known bank (`kit::spell_load`), products banked between loads.
//! Teleports and bones to bananas are spellbook buttons (`[if_button,
//! magic:...]` in `teleport.rs2`, `convert_bones.rs2`), enchants a cast
//! on the carried jewel (`OPHELDT`). D18 (2026-11-16): a teleport order
//! with a goal (`--level`, count, budget) casts until it, a magic climb;
//! a goalless one casts once (travel).
//!
//! Process: poste aucun (sort) · action enchanter/téléporter ·
//! interface sort · intrants amulette/os + runes · produits amulettes
//! enchantées / destination / bananes.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::magic::{cast_on_held, wield_staff_for, CastPacer};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::spell_load::{run_casts, stock_for, SpellLoad};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const MAGIC_STAT: u8 = 6;
/// F2P enchants from the server data (`magic_spells.dbrow` convertobj
/// rows, `config_recipes` spell kind): spell, level, input, product.
/// Only F2P spells whose product is no members object
/// (`enchant.rs2`: `oc_members($final_obj) & map_members = false`
/// refuses). Live admin 2026-11-15: a hand list of four amulets refused
/// the sapphire necklace the server enchants into a games necklace.
pub fn enchants() -> Vec<(String, i64, String, String)> {
    knowledge::script_sources::config_recipes()
        .iter()
        .filter(|row| row.kind == "spell")
        .filter_map(|row| {
            let spell = row.spell.clone()?;
            let entry = knowledge::spells::by_name(&spell).into_iter().next()?;
            let input = row.inputs.first()?.clone();
            let f2p = |name: &str| knowledge::objs_by_name(name).into_iter().next().is_some_and(|obj| obj.members == Some(false));
            (!entry.members && f2p(&input) && f2p(&row.output)).then(|| (spell, entry.level, input, row.output.clone()))
        })
        .collect()
}
/// F2P teleports from the spell dump (`*_teleport`, members rows out):
/// spell and level, lowest first (Varrock 25, Lumbridge 31, Falador 37).
pub fn teleports() -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = knowledge::spells::all_spells()
        .into_iter()
        .filter(|row| !row.members && row.level >= 1 && row.spell.ends_with("_teleport") && !row.runes.is_empty())
        .map(|row| (row.spell, row.level))
        .collect();
    out.sort_by_key(|(_, level)| *level);
    out
}
pub const BONES_TO_BANANAS: &str = "bones_to_bananas";
/// The only bones the spell converts (`convert_bones.rs2`).
pub const BANANA_BONES: &str = "bones";

/// Runes of a spell from the dump (case-insensitive debugname).
pub fn spell_runes(spell: &str) -> Vec<(String, i64)> {
    knowledge::spells::by_name(spell).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Enchant row for a jewel (an unstrung amulet reads strung): spell,
/// level, product.
pub fn enchant_for(jewel: &str) -> Option<(String, i64, String)> {
    let strung = strung_input(jewel);
    enchants().into_iter().find(|(_, _, input, _)| input.eq_ignore_ascii_case(&strung)).map(|(spell, level, _, product)| (spell, level, product))
}

/// Strung input of an (un)strung amulet name, mirroring `enchant_for`.
fn strung_input(wanted: &str) -> String {
    wanted.strip_prefix("unstrung_").map(|rest| format!("strung_{rest}")).unwrap_or_else(|| wanted.to_string())
}

/// Required level of a teleport spell (`None` for anything else).
pub fn teleport_level(name: &str) -> Option<i64> {
    teleports().into_iter().find(|(spell, _)| spell.eq_ignore_ascii_case(name)).map(|(_, level)| level)
}

/// True when the order names a goal the casts climb to (a teleport
/// training run): a count, a level, an XP amount or a budget.
fn has_goal(spec: &crate::kit::cycle::TaskSpec) -> bool {
    let targets = &spec.targets;
    targets.count.is_some_and(|count| count > 0) || targets.level_skill.is_some() || targets.xp_skill.is_some() || spec.budget_max_ticks > 0
}

fn magic_xp(bot: &Bot) -> i32 {
    bot.world.stats.get(&MAGIC_STAT).map(|(_, xp)| *xp).unwrap_or(0)
}

/// Cast in the open session: `enchant_teleport <spell|amulet> [count]
/// [--want ITEM N] [--level magic N]`. Enchants and bananas cast on the
/// carried target (`OPHELDT`); teleports press the spellbook button
/// (`IF_BUTTON`) and land the bot elsewhere. Runes and targets come from
/// the pack, then the known bank (`kit::spell_load`: a staff pays its
/// rune, products banked between loads); `Blocked` names exactly what
/// pack + bank lack.
pub fn run_enchant_teleport(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("enchant_teleport", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let keep = crate::kit::cycle::resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, MAGIC_STAT, 1);
    let wanted = resource.as_deref().ok_or_else(|| ScriptError::Usage(format!("enchant_teleport <spell|amulet> [count], nothing named at {:?}", bot.position())))?;
    let (spell, need, input) = if let Some((spell, need, _)) = enchant_for(wanted) {
        (spell, need, strung_input(wanted))
    } else if let Some(need) = teleport_level(wanted) {
        (wanted.to_lowercase(), need, String::new())
    } else if wanted.eq_ignore_ascii_case(BONES_TO_BANANAS) {
        let need = knowledge::spells::by_name(BONES_TO_BANANAS).into_iter().next().map(|row| row.level).unwrap_or(15);
        (BONES_TO_BANANAS.to_string(), need, BANANA_BONES.to_string())
    } else {
        return Err(ScriptError::Usage(format!("no F2P enchant, teleport or {BONES_TO_BANANAS} named {wanted:?} at {:?}", bot.position())));
    };
    if level < need {
        return Err(crate::kit::cycle::blocked_level("magic", i64::from(need)));
    }
    let runes = spell_runes(&spell);
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{spell} has no runes in the dump at {:?}", bot.position())));
    }
    let target_obj = if input.is_empty() { None } else { Some(obj_id(bot, &input)?) };
    let inputs = if input.is_empty() { Vec::new() } else { vec![(input.clone(), 1)] };
    let mut load = SpellLoad::new(&spell, runes, inputs);
    load.whole_pack = spell == BONES_TO_BANANAS;
    // A carried staff of the cast's element goes on before any sizing.
    wield_staff_for(bot, &load.runes)?;
    let mut pacer = CastPacer::new(&spell);
    // A goalless teleport moves the bot once (travel); with a goal it
    // casts until it (a magic climb, D18), runes refilled between loads.
    let Some(target) = target_obj else {
        if !has_goal(&spec) {
            stock_for(bot, &load, &keep, 1)?;
            cast_button(bot, &mut pacer)?;
            bot.log(&format!("ok {spell} x1"));
            return Ok(0);
        }
        let (done, stop) = run_casts(bot, &spec, &load, &keep, |cast_bot| cast_button(cast_bot, &mut pacer))?;
        bot.log(&format!("ok {spell} x{done}"));
        return stop.map_or(Ok(0), crate::kit::cycle::exit_code);
    };
    let (done, stop) = if spell == BONES_TO_BANANAS {
        // The button converts every carried bone at once.
        run_casts(bot, &spec, &load, &keep, |cast_bot| {
            let had = pack_total(cast_bot, target);
            pacer.cast_button(cast_bot, |seen| pack_total(seen, target) < had)
        })?
    } else {
        run_casts(bot, &spec, &load, &keep, |cast_bot| cast_one(cast_bot, &spell, target))?
    };
    bot.log(&format!("ok {spell} x{done}"));
    stop.map_or(Ok(0), crate::kit::cycle::exit_code)
}

/// One spellbook-button cast (teleport): landed when its XP shows.
fn cast_button(bot: &mut Bot, pacer: &mut CastPacer) -> Result<(), ScriptError> {
    let before = magic_xp(bot);
    pacer.cast_button(bot, |seen| magic_xp(seen) > before)
}

/// Ticks to wait out one cast (server delays plus the convert tick).
pub const CAST_TICKS: u64 = 40;

/// One enchant on the carried jewel: the input count moves. Anything
/// quiet past the budget is loud.
fn cast_one(bot: &mut Bot, spell: &str, obj: u16) -> Result<(), ScriptError> {
    let had = pack_total(bot, obj);
    cast_on_held(bot, spell, obj)?;
    let deadline = bot.world.ticks + CAST_TICKS;
    while bot.world.ticks < deadline {
        bot.pump(2)?;
        if pack_total(bot, obj) != had {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("{spell} never landed at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{enchant_for, enchants, has_goal, run_enchant_teleport, spell_runes, teleport_level, teleports, BONES_TO_BANANAS, CAST_TICKS, MAGIC_STAT};
    use crate::kit::pack::{obj_id, pack_total};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Fund the pack through the live tables (pack com + slots).
    fn fund_pack(bot: &mut Bot, stock: &[(u8, &str, u32)]) {
        let com = (bot.tables.com_id)("inventory:inv").expect("pack com in dumps");
        let slots: std::collections::HashMap<u8, (u16, u32)> = stock
            .iter()
            .map(|(slot, name, count)| (*slot, (obj_id(bot, name).expect("funded obj resolves"), *count)))
            .collect();
        bot.world.inv.insert(com, slots);
        bot.world.inv_cap.insert(com, 28);
    }

    /// Runes starve loud: an unfunded pack blocks with the rune names.
    #[test]
    fn unfunded_pack_blocks_on_runes() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (7, 0));
        fund_pack(&mut bot, &[(0, "strung_sapphire_amulet", 1)]);
        let error = run_enchant_teleport(&mut bot, &["strung_sapphire_amulet".to_string()])
            .expect_err("no runes, no cast");
        assert!(matches!(error, crate::ScriptError::Blocked { .. }), "blocked, got {error}");
        assert!(format!("{error}").contains("cosmicrune"), "names the rune, got {error}");
    }

    /// bd23dqpq (live 2026-11-13, blocked four times "missing:
    /// cosmicrune, waterrune"): 2 cosmic and 67 water in the bank, the
    /// amulet carried. The run goes to the bank (offline: the walk fails
    /// typed), never `blocked` on runes the bank holds.
    #[test]
    fn banked_runes_send_the_run_to_the_bank() {
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("strung_sapphire_amulet", 1)], &[("cosmicrune", 2), ("waterrune", 67)]);
        bot.world.stats.insert(MAGIC_STAT, (7, 0));
        bot.world.x = 3222;
        bot.world.z = 3218;
        let error = run_enchant_teleport(&mut bot, &["strung_sapphire_amulet".to_string()]).expect_err("offline bank walk fails");
        assert!(!matches!(error, crate::ScriptError::Blocked { .. }), "the bank holds the runes, got {error}");
    }

    /// Pack and bank both short: blocked with exact names only.
    #[test]
    fn short_pack_and_bank_block_with_exact_names() {
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("strung_sapphire_amulet", 1)], &[("waterrune", 67)]);
        bot.world.stats.insert(MAGIC_STAT, (7, 0));
        match run_enchant_teleport(&mut bot, &["strung_sapphire_amulet".to_string()]) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["cosmicrune".to_string()]),
            other => panic!("blocked on cosmic only, got {other:?}"),
        }
    }

    /// A funded enchant reaches the send brick: dead socket fails `Sdk`,
    /// never the old dette `Usage`.
    #[test]
    fn funded_enchant_reaches_the_send_brick() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (7, 0));
        // cosmic + water for enchant_lvl1 (dump runes, sorted check below).
        let mut runes = spell_runes("enchant_lvl1");
        runes.sort();
        fund_pack(&mut bot, &[(0, "strung_sapphire_amulet", 1), (1, "cosmicrune", 1), (2, "waterrune", 1)]);
        let input = obj_id(&bot, "strung_sapphire_amulet").expect("input resolves");
        let error = run_enchant_teleport(&mut bot, &["strung_sapphire_amulet".to_string()])
            .expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        assert_eq!(pack_total(&bot, input), 1, "nothing converted offline");
        assert!(CAST_TICKS >= 3, "covers the server delays");
    }

    /// A funded teleport reaches the button press: dead socket fails
    /// `Sdk`, never the old dette `Usage`.
    #[test]
    fn funded_teleport_reaches_the_send_brick() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (25, 0));
        fund_pack(&mut bot, &[(0, "airrune", 3), (1, "lawrune", 1), (2, "firerune", 1)]);
        let error = run_enchant_teleport(&mut bot, &["varrock_teleport".to_string()]).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
    }

    /// Catalogue §5.10 ladder matches the spell dump: levels, cosmic
    /// rune counts and F2P flags.
    #[test]
    fn enchant_ladder_matches_the_spell_dump() {
        let amulets: Vec<i64> = ["strung_sapphire_amulet", "strung_emerald_amulet", "strung_ruby_amulet", "strung_diamond_amulet"].iter().map(|amulet| enchant_for(amulet).expect(amulet).1).collect();
        assert_eq!(amulets, vec![7, 27, 49, 57]);
        for (spell, level, _, _) in enchants() {
            let rows = lifescape_gamedata::spells::by_name(&spell);
            assert_eq!(rows.len(), 1, "{spell} resolves once");
            assert_eq!(rows[0].level, level, "{spell} level");
            assert!(!rows[0].members, "{spell} F2P");
            assert!(spell_runes(&spell).iter().any(|(name, _)| name == "cosmicrune"), "{spell} eats cosmic, got {:?}", spell_runes(&spell));
        }
        assert_eq!(enchant_for("strung_sapphire_amulet"), Some(("enchant_lvl1".to_string(), 7, "amulet_of_magic".to_string())));
        assert_eq!(enchant_for("unstrung_emerald_amulet"), Some(("enchant_lvl2".to_string(), 27, "amulet_of_defence".to_string())), "unstrung maps too");
        assert_eq!(enchant_for("sapphire_necklace").map(|row| row.2), Some("necklace_of_minigames_8".to_string()), "the server enchants it in F2P");
        assert!(enchant_for("sapphire_ring").is_none(), "ring of recoil is members");
        assert!(enchant_for("strung_gold_amulet").is_none(), "gold never enchants");
        assert!(enchant_for("coins").is_none(), "junk never enchants");
    }

    /// Teleports and bones to bananas: levels and runes from the dump.
    #[test]
    fn teleports_and_bones_match_the_dump() {
        assert_eq!(teleports(), vec![("varrock_teleport".to_string(), 25), ("lumbridge_teleport".to_string(), 31), ("falador_teleport".to_string(), 37)]);
        assert_eq!(teleport_level("varrock_teleport"), Some(25));
        assert_eq!(teleport_level("FALADOR_TELEPORT"), Some(37), "case-insensitive");
        assert_eq!(teleport_level("camelot_teleport"), None, "members never teleports");
        assert!(spell_runes("varrock_teleport").contains(&("lawrune".to_string(), 1)), "teleports eat law");
        let bones = lifescape_gamedata::spells::by_name(BONES_TO_BANANAS);
        assert_eq!(bones.len(), 1);
        assert_eq!((bones[0].level, bones[0].members), (15, false));
        let mut bones_runes = spell_runes(BONES_TO_BANANAS);
        bones_runes.sort();
        assert_eq!(bones_runes, vec![("earthrune".to_string(), 2), ("naturerune".to_string(), 1), ("waterrune".to_string(), 2)]);
    }

    /// D18: a teleport order with a goal climbs (count, level, budget);
    /// a bare one travels once.
    #[test]
    fn a_goal_turns_teleports_into_a_climb() {
        let parse = |args: &[&str]| crate::kit::cycle::parse_cli("enchant_teleport", &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()).expect("parses");
        assert!(!has_goal(&parse(&[])), "travel");
        assert!(has_goal(&parse(&["--level", "magic", "30"])));
        assert!(has_goal(&parse(&["40"])));
        assert!(has_goal(&parse(&["--budget-max", "6000"])));
    }

    /// A funded teleport climb and bananas reach the button press: dead
    /// socket fails `Sdk` (bananas never cast on a held bone: the server
    /// only has `[if_button,magic:bones_to_bananas]`).
    #[test]
    fn funded_climbs_reach_the_button() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (25, 0));
        fund_pack(&mut bot, &[(0, "airrune", 30), (1, "lawrune", 10), (2, "firerune", 10)]);
        let error = run_enchant_teleport(&mut bot, &["varrock_teleport".to_string(), "--level".to_string(), "magic".to_string(), "30".to_string()]).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (15, 0));
        fund_pack(&mut bot, &[(0, "bones", 1), (1, "earthrune", 2), (2, "waterrune", 2), (3, "naturerune", 1)]);
        let error = run_enchant_teleport(&mut bot, &[BONES_TO_BANANAS.to_string(), "1".to_string()]).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
    }

    /// Every enchant product exists in the object dump.
    #[test]
    fn enchant_products_exist() {
        for (_, _, input, product) in enchants() {
            assert!(!lifescape_gamedata::objs_by_name(&input).is_empty(), "{input} exists");
            assert!(!lifescape_gamedata::objs_by_name(&product).is_empty(), "{product} exists");
        }
    }
}
