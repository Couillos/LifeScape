//! Telegrab the wine of Zamorak (M): 33 Magic, air + law runes, the wine
//! off the Chaos Temple altar. Declaration + walk/detect particularités
//! on the kit; the cast itself is a typed debt (below). No station:
//! one F2P spawn (catalogue §5.10, P1).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{parse_cli, resolve_keep};
use crate::kit::magic::{cast_on_ground, wield_staff_for};
use crate::kit::pack::obj_id;
use crate::kit::spell_load::{stock_for, SpellLoad};
use crate::kit::survival::{check_events, is_walk_abort, learn_level, wait_pack_streamed};
use crate::skill_walk::walk_checked;
use crate::{ScriptError, ScriptResult};

/// Magic stat and the spell level (`skill_magic` dbrow).
pub const MAGIC_STAT: u8 = 6;
pub const NEED_LEVEL: u8 = 33;
/// One cast eats one air and one law rune (dump dbrow).
pub const RUNE_AIR: &str = "airrune";
pub const RUNE_LAW: &str = "lawrune";
/// Spell debugname (`magic:telekinetic_grab` button).
pub const SPELL: &str = "telekinetic_grab";
/// Product and its altar tile (dump spawn, Wilderness temple).
pub const PRODUCT: &str = "wine_of_zamorak";
pub const WINE_SPAWN: (i32, i32, i32) = (2930, 3515, 0);
/// Spell XP per cast, in real points (the dump counts tenths, like cook).
pub const XP_PER_CAST: f64 = 43.0;
/// Tick budget per cast: the server delays plus the altar approach.
pub const CAST_TICKS: u64 = 30;
/// Empty-altar checks before the run ends loud (respawn unknown).
pub const WINE_EMPTY_LIMIT: u32 = 3;
/// Approach budget for the altar stand.
pub const ALTAR_WALK_TICKS: u64 = 6000;

/// One telegrab's runes (air + law, dump row): stocked from the pack,
/// then the known bank (`kit::spell_load`), a staff of air paying the air.
pub fn telegrab_load() -> SpellLoad {
    let runes = lifescape_gamedata::spells::by_name(SPELL).into_iter().flat_map(|entry| entry.runes).collect();
    SpellLoad::new(SPELL, runes, Vec::new())
}

/// Cast on the nearest visible wine, then pump until it leaves the
/// ground (the server answers with the item removal + XP drop).
fn cast_telegrab(bot: &mut Bot, wine: u16) -> Result<(), ScriptError> {
    let ground = bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, 15, Some(wine));
    let ((tile, _), _) = ground
        .iter()
        .min_by_key(|((tile, _), _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs())
        .ok_or_else(|| ScriptError::Failed(format!("wine gone at {WINE_SPAWN:?} from {:?}", bot.position())))?;
    cast_on_ground(bot, SPELL, tile.0, tile.1, wine)?;
    for _ in 0..CAST_TICKS {
        bot.pump(1)?;
        if !wine_on_floor(bot, wine) {
            bot.log("ok wine grabbed");
            return Ok(());
        }
    }
    Err(ScriptError::Usage(format!("telegrab fizzled at {WINE_SPAWN:?} from {:?}", bot.position())))
}

/// Stand on the altar tile or an orthogonal neighbour (the altar loc
/// itself may be unwalkable).
fn walk_to_stand(bot: &mut Bot) -> Result<(), ScriptError> {
    let (x, z, level) = WINE_SPAWN;
    if bot.world.level != level {
        return Err(ScriptError::Usage(format!("wine is on level {level}, bot at {:?}", bot.position())));
    }
    if walk_checked(bot, x, z, ALTAR_WALK_TICKS)? {
        return Ok(());
    }
    for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        match walk_checked(bot, x + dx, z + dz, ALTAR_WALK_TICKS / 4) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) if is_walk_abort(&error) => return Err(error),
            Err(_) => {}
        }
    }
    Err(ScriptError::Failed(format!("no altar stand near ({x},{z}) from {:?}", bot.position())))
}

/// True when the wine is on the floor nearby.
fn wine_on_floor(bot: &Bot, wine: u16) -> bool {
    bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, 15, Some(wine)).values().sum::<u32>() > 0
}

/// Telegrab wine in the open session: `telegrab_wine [count]`.
pub fn run_telegrab_wine(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = parse_cli("telegrab_wine", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, MAGIC_STAT, 1);
    if level < i64::from(NEED_LEVEL) {
        return Err(crate::kit::cycle::blocked_level("magic", i64::from(i64::from(NEED_LEVEL))));
    }
    let wine = obj_id(bot, PRODUCT)?;
    let load = telegrab_load();
    wield_staff_for(bot, &load.runes)?;
    // A market pause in the respawn wait banks the runes and leaves the
    // bot at the Varrock square (`kit::site::reload_after_pause`): runes
    // and stand again, never a cast from the square.
    let mut restarts = 0u32;
    loop {
        let pauses_at_load = bot.pauses_done;
        stock_for(bot, &load, &keep, 1)?;
        walk_to_stand(bot)?;
        for _ in 0..WINE_EMPTY_LIMIT {
            if wine_on_floor(bot, wine) {
                break;
            }
            bot.log("altar empty, waiting for the respawn");
            for _ in 0..10 {
                bot.pump(2)?;
            }
        }
        if !crate::kit::site::reload_after_pause(pauses_at_load, bot.pauses_done, restarts) {
            break;
        }
        restarts += 1;
        bot.log(&format!("telegrab: a market pause left us at {:?}, runes and stand again", bot.position()));
    }
    if !wine_on_floor(bot, wine) {
        return Err(lost_wine());
    }
    bot.log(&format!("ok casting telegrab near {WINE_SPAWN:?} (magic={level})"));
    bot.intend("telegrabbing the wine of Zamorak");
    let before = crate::kit::pack::pack_total(bot, wine);
    cast_telegrab(bot, wine)?;
    // The single spawn is fought over by the fleet: a wine that left the
    // floor without entering the pack went to someone else (live
    // 2026-11-22: 21 runs "goal_reached" with no XP, skill banned).
    bot.pump(3)?;
    if crate::kit::pack::pack_total(bot, wine) <= before {
        return Err(lost_wine());
    }
    Ok(0)
}

/// The altar's wine went to another player: the site is contended, the
/// brain cools it without a failure (`kit::site::CONTENDED_TAG`).
pub fn lost_wine() -> ScriptError {
    let (x, z, level) = WINE_SPAWN;
    ScriptError::Blocked { missing: vec![format!("{}{x},{z},{level}", crate::kit::site::CONTENDED_TAG)] }
}

#[cfg(test)]
mod tests {
    use super::{CAST_TICKS, MAGIC_STAT, NEED_LEVEL, PRODUCT, RUNE_AIR, RUNE_LAW, WINE_EMPTY_LIMIT, WINE_SPAWN, XP_PER_CAST};
    use crate::kit::pack::obj_id;
    use crate::kit::survival::learn_level;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Live 2026-11-22: a lost wine is contention, never a bare success.
    #[test]
    fn a_lost_wine_is_a_contended_block() {
        match super::lost_wine() {
            crate::ScriptError::Blocked { missing } => assert_eq!(missing, vec!["contended:2930,3515,0".to_string()]),
            other => panic!("{other:?}"),
        }
    }

    /// Declaration matches the dump spell row: level 33, air + law, F2P,
    /// 430 tenths = 43.0 XP.
    #[test]
    fn declaration_matches_the_spell_row() {
        let row = knowledge::spells::by_name("telekinetic_grab").into_iter().next().expect("telegrab row");
        assert_eq!(row.level, i64::from(NEED_LEVEL), "level 33");
        assert!(!row.members, "F2P");
        assert!(row.runes.iter().any(|(name, count)| name.as_str() == RUNE_AIR && *count == 1), "air x1, got {:?}", row.runes);
        assert!(row.runes.iter().any(|(name, count)| name.as_str() == RUNE_LAW && *count == 1), "law x1, got {:?}", row.runes);
        assert_eq!(f64::from(u32::try_from(row.xp).expect("xp fits")) / 10.0, XP_PER_CAST, "tenths to points");
        let bot = offline_bot();
        obj_id(&bot, RUNE_AIR).expect("airrune in tables");
        obj_id(&bot, RUNE_LAW).expect("lawrune in tables");
        obj_id(&bot, PRODUCT).expect("wine in tables");
        assert_eq!(lifescape_sdk::provenance::skill_id("magic"), Some(MAGIC_STAT), "magic stat");
    }

    /// The altar spawn exists in the dump (the single F2P wine).
    #[test]
    fn altar_spawn_exists_in_the_dump() {
        let wine = knowledge::objs_by_name(PRODUCT).into_iter().next().expect("wine row");
        let (x, z, level) = WINE_SPAWN;
        let found = knowledge::spawns::all_obj_spawns(Some(wine.obj_id))
            .into_iter()
            .any(|spawn| spawn.x == i64::from(x) && spawn.z == i64::from(z) && spawn.level == i64::from(level));
        assert!(found, "wine spawns at {WINE_SPAWN:?}");
    }

    /// Level gate reads the live magic level (32 refused, 33 passes).
    #[test]
    fn level_gate_reads_live_magic() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (32, 0));
        assert!(learn_level(&bot, MAGIC_STAT, 1) < i64::from(NEED_LEVEL), "32 refused");
        bot.world.stats.insert(MAGIC_STAT, (33, 0));
        assert!(learn_level(&bot, MAGIC_STAT, 1) >= i64::from(NEED_LEVEL), "33 passes");
    }

    /// Rune shortfall names exactly what pack + bank lack (the brain's
    /// obtain task): air banked counts, a worn staff of air pays it, law
    /// nowhere is named alone; the pack funds no cast until the refill.
    #[test]
    fn shortfall_counts_bank_and_staff() {
        let load = super::telegrab_load();
        let bot = crate::kit::spell_load::tests::stocked_bot(&[], &[(RUNE_AIR, 5), (RUNE_LAW, 5)]);
        assert!(load.shortfall(&bot).is_empty(), "the bank funds the cast");
        assert_eq!(load.pack_casts(&bot), 0, "a bank trip first");
        let bot = crate::kit::spell_load::tests::stocked_bot(&[(RUNE_AIR, 1)], &[]);
        assert_eq!(load.shortfall(&bot), vec![RUNE_LAW.to_string()], "law missing");
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[(RUNE_LAW, 1)], &[]);
        let staff = obj_id(&bot, "staff_of_air").expect("air staff");
        let worn_com = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME).expect("worn com");
        bot.world.inv.insert(worn_com, [(3u8, (staff, 1u32))].into_iter().collect());
        assert!(load.shortfall(&bot).is_empty(), "the staff pays the air");
        assert_eq!(load.pack_casts(&bot), 1, "law alone funds the cast");
    }

    /// Cast reaches the send brick: a visible wine on a dead socket
    /// fails `Sdk` (the packet cannot leave), never the old dette
    /// `Usage` — and no wine at all stays a loud `Usage`.
    #[test]
    fn cast_reaches_the_send_brick() {
        let mut bot = offline_bot();
        let wine = obj_id(&bot, PRODUCT).expect("wine resolves");
        bot.world.x = 2930;
        bot.world.z = 3514;
        bot.world.level = 0;
        let error = super::cast_telegrab(&mut bot, wine).expect_err("no wine, no cast");
        assert!(format!("{error}").contains("wine gone"), "loud usage, got {error}");
        bot.world.objs.insert(((2930, 3515, 0), wine), Some(1));
        let error = super::cast_telegrab(&mut bot, wine).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        assert!(!format!("{error}").contains("dette"), "the dette Usage is gone: {error}");
    }

    /// Budget: one cast fits in tens of ticks (server delays + approach),
    /// and an empty altar ends after a few checks, never idles.
    #[test]
    fn budget_bounds_are_sane() {
        assert!(CAST_TICKS >= 3, "covers the server delays, got {CAST_TICKS}");
        assert!(CAST_TICKS <= 600, "a cast is quick, got {CAST_TICKS}");
        assert!(WINE_EMPTY_LIMIT <= 5, "empty altar ends fast: {WINE_EMPTY_LIMIT}");
    }
}
