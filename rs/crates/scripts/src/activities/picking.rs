//! Picking (R): wheat, potatoes, cabbage and onions off F2P fields.
//! Declaration + loc-harvest particularités on the kit: nearest field
//! tile, one `watch_stand` click each (picking is instant, the produce is
//! the ack), banked loads through `kit::cycle`. No level, no tool: 597
//! wheat, 268 potato, 189 cabbage and 27 onion tiles (stations P2.1,
//! catalogue §6, P2, epoch P2).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bans::{BanSet, GONE_BAN_TICKS};
use crate::kit::bank::{bank_trip_resilient, nearest_bank};
use crate::kit::cycle::{parse_cli, resolve_keep, run, StepControl};
use crate::kit::harvest::{watch_stand, HarvestSource, HarvestTarget, WorkOutcome};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, is_walk_abort, wait_pack_streamed};
use crate::skill_walk::walk_checked;
use crate::{ScriptError, ScriptResult};

/// Catalogue §6 epoch and archetype (P2 field harvest, R).
pub const EPOCH: &str = "P2";
pub const ARCH: &str = "R";
/// Field kind to picked product (wheat threshes to grain, the mill input).
pub const CROPS: [(&str, &str); 4] =
    [("wheat", "grain"), ("potato", "potato"), ("cabbage", "cabbage"), ("onion", "onion")];
/// Server pick op on field locs (`pickables.rs2`: `[oploc2,wheat]`…).
pub const PICK_OP: u8 = 2;
/// Start-of-work acks: none, the produce itself is the ack.
pub const PICK_ACKS: [&str; 0] = [];
/// Empty fields before the run ends loud instead of idling.
pub const EMPTY_LIMIT: u32 = 5;
/// Approach budget per field tile.
pub const FIELD_WALK_TICKS: u64 = 6000;

/// Picked product for a field kind (`grain` aliases wheat, like the dump).
pub fn crop_product(kind: &str) -> Option<&'static str> {
    if kind.eq_ignore_ascii_case("grain") {
        return Some("grain");
    }
    CROPS.iter().find(|(field, _)| field.eq_ignore_ascii_case(kind)).map(|(_, product)| *product)
}

/// One click, one tile: pick op 2, watch the produce land. A picked tile
/// depletes (the loc streams away); an unstreamed tile reads as live, so
/// the click itself arbitrates (never assume depletion).
struct PickSource {
    loc: u16,
    x: i32,
    z: i32,
}

impl HarvestSource for PickSource {
    fn acks(&self) -> &[&str] {
        &PICK_ACKS
    }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<HarvestTarget, ScriptError> {
        bot.log(&format!("picking field at ({x},{z})"));
        bot.intend("picking a crop");
        bot.op_loc(PICK_OP, x, z, self.loc)?;
        Ok(HarvestTarget::Loc { loc: self.loc })
    }

    fn slot_gone(&self, _bot: &mut Bot, _target: &HarvestTarget) -> bool {
        false
    }

    fn slot_moved(&self, _bot: &mut Bot, _target: &HarvestTarget) -> bool {
        false
    }

    fn target_live(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        bot.world.loc_at(self.x, self.z, bot.world.level).is_none_or(|(id, _, _)| id == self.loc)
    }

    fn depleted_now(&self, _bot: &mut Bot, _x: i32, _z: i32, _silences: u32) -> bool {
        false
    }

    /// Instant action: one quiet round without produce means the click never
    /// landed — move on.
    fn unacked_rounds(&self) -> u32 {
        1
    }

    fn reprobe(&self, _acked: bool, _live: bool, _since_probe: u64) -> bool {
        false
    }
}

/// Nearest unbanned tile of one field kind, occupation-dodged. Station rows
/// carry their loc id, so no live resolution is needed before the click.
fn pick_field(bot: &mut Bot, kind: &str, banned: &BanSet, now: u64) -> Option<(i32, i32, u16)> {
    let mut cands: Vec<(i64, (i32, i32, u16))> = Vec::new();
    for station in lifescape_gamedata::stations::stations_of(kind) {
        let Ok(loc) = u16::try_from(station.loc_id) else {
            continue;
        };
        if station.level != i64::from(bot.world.level) {
            continue;
        }
        let tile = (station.x as i32, station.z as i32);
        if banned.is_banned(tile, now) {
            continue;
        }
        if (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs() > 400 {
            continue;
        }
        cands.push((i64::from((tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs()), (tile.0, tile.1, loc)));
    }
    let tiles: Vec<(i64, (i32, i32))> = cands.iter().map(|(dist, (x, z, _))| (*dist, (*x, *z))).collect();
    let free = crate::kit::spots::drop_occupied(bot, tiles, crate::kit::spots::OCCUPIED_RADIUS);
    free.into_iter()
        .filter_map(|(_, (x, z))| cands.iter().find(|(_, tile)| (tile.0, tile.1) == (x, z)).copied())
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, tile)| tile)
}

/// Work one tile: walk over (fields never block), click, watch the crop land.
fn work_field(bot: &mut Bot, x: i32, z: i32, loc: u16, product: u16) -> Result<(WorkOutcome, u32), ScriptError> {
    match walk_checked(bot, x, z, FIELD_WALK_TICKS) {
        Ok(true) => {}
        Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
        Err(error) if is_walk_abort(&error) => return Err(error),
        Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
    }
    let had = pack_total(bot, product);
    let mut source = PickSource { loc, x, z };
    watch_stand(bot, &mut source, x, z, &[product], had)
}

/// Pick banked loads in the open session: `picking [crop] [count]`
/// (wheat when omitted, 0 = forever).
pub fn run_picking(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = parse_cli("picking", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let kind = resource.as_deref().unwrap_or("wheat");
    let product_name = crop_product(kind)
        .ok_or_else(|| ScriptError::Usage(format!("no F2P field crop named {kind:?} at {:?}", bot.position())))?;
    let product = obj_id(bot, product_name)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut banned = BanSet::new();
    let mut empty = 0u32;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        bank = nearest_bank(step_bot)?;
        let now = step_bot.world.ticks;
        let Some((x, z, loc)) = pick_field(step_bot, kind, &banned, now) else {
            empty += 1;
            if empty >= EMPTY_LIMIT {
                return Err(ScriptError::Failed(format!("no {kind} field near {:?}", step_bot.position())));
            }
            step_bot.log("field empty, waiting for respawns");
            for _ in 0..10 {
                step_bot.pump(2)?;
            }
            return Ok(StepControl::Continue);
        };
        empty = 0;
        let (outcome, _) = work_field(step_bot, x, z, loc, product)?;
        match outcome {
            WorkOutcome::Gone | WorkOutcome::Moved | WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot => {
                banned.ban_for((x, z), step_bot.world.ticks, GONE_BAN_TICKS);
                step_bot.log("field spent, re-picking");
                return Ok(StepControl::Continue);
            }
            WorkOutcome::NoTool | WorkOutcome::NoBait => {
                return Err(ScriptError::Failed(format!("picking {kind} refused at {:?}", step_bot.position())));
            }
            WorkOutcome::Threat | WorkOutcome::Blocked | WorkOutcome::LevelLocked => {
                return Err(ScriptError::Failed(format!("picking stopped ({outcome:?}) at {:?}", step_bot.position())));
            }
            WorkOutcome::Full => {}
        }
        if free_slots(step_bot) > 0 {
            step_bot.log("crop picked, same load");
            return Ok(StepControl::Continue);
        }
        let keep: Vec<u16> = task_keep.clone();
        match bank_trip_resilient(step_bot, &mut bank, &keep, 0, &[product]) {
            Ok(_) => {}
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died on the bank trip, picking again");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
        }
        step_bot.log(&format!("ok load {} banked", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

#[cfg(test)]
mod tests {
    use super::{crop_product, pick_field, ARCH, CROPS, EPOCH, EMPTY_LIMIT, PICK_ACKS, PICK_OP};
    use crate::kit::bans::BanSet;
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Declaration: epoch P2, archetype R, four crops with wheat aliasing
    /// grain (the mill input).
    #[test]
    fn crops_map_to_their_products() {
        assert_eq!((EPOCH, ARCH), ("P2", "R"));
        assert_eq!(CROPS.len(), 4);
        assert_eq!(crop_product("wheat"), Some("grain"));
        assert_eq!(crop_product("grain"), Some("grain"), "wheat alias");
        assert_eq!(crop_product("WHEAT"), Some("grain"), "case-insensitive");
        assert_eq!(crop_product("potato"), Some("potato"));
        assert_eq!(crop_product("cabbage"), Some("cabbage"));
        assert_eq!(crop_product("onion"), Some("onion"));
        assert_eq!(crop_product("flax"), None, "members crop is no tier");
        assert_eq!(crop_product("coins"), None, "junk never picks");
        assert_eq!(PICK_OP, 2, "pickables.rs2 oploc2");
        assert!(PICK_ACKS.is_empty(), "the produce is the ack");
    }

    /// Products resolve and are F2P (D63: the dump flag, never spawn flags).
    #[test]
    fn products_are_f2p() {
        let bot = offline_bot();
        for (_, product) in CROPS {
            obj_id(&bot, product).expect("product in tables");
            let row = knowledge::objs_by_name(product).into_iter().next().expect("dump row");
            assert_eq!(row.members, Some(false), "{product} is F2P, got {:?}", row.members);
        }
    }

    /// Field locs carry Pick at op 2; station counts match the catalogue.
    #[test]
    fn fields_match_server_and_catalogue() {
        for (kind, _) in CROPS {
            let loc = knowledge::all_locs().into_iter().find(|entry| entry.name.as_deref() == Some(kind)).expect("field loc");
            assert!(matches!(loc.ops.get(1), Some(Some(op)) if *op == "Pick"), "{kind} picks at op 2, got {:?}", loc.ops);
        }
        assert_eq!(knowledge::stations::stations_of("wheat").len(), 597, "wheat tiles");
        assert_eq!(knowledge::stations::stations_of("potato").len(), 268, "potato tiles");
        assert_eq!(knowledge::stations::stations_of("cabbage").len(), 189, "cabbage tiles");
        assert_eq!(knowledge::stations::stations_of("onion").len(), 27, "onion tiles");
    }

    /// Pick: nearest unbanned wheat tile wins, banned ones are skipped.
    #[test]
    fn pick_skips_banned_tiles() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let near = knowledge::stations::stations_of("wheat").into_iter().find(|station| station.level == 0).expect("a wheat tile");
        bot.world.x = near.x as i32;
        bot.world.z = near.z as i32;
        let banned = BanSet::new();
        let first = pick_field(&mut bot, "wheat", &banned, 0).expect("a wheat tile picks");
        assert_eq!((first.0, first.1), (near.x as i32, near.z as i32), "underfoot tile wins");
        let mut banned = BanSet::new();
        banned.ban_for((near.x as i32, near.z as i32), 0, 10_000);
        let second = pick_field(&mut bot, "wheat", &banned, 0).expect("another tile picks");
        assert!((second.0, second.1) != (near.x as i32, near.z as i32), "banned skipped, got {second:?}");
    }

    /// Click is instant (one quiet round, no re-click), an empty field ends
    /// the run after a few waits, never idles.
    #[test]
    fn click_is_instant_and_empty_ends_fast() {
        let source = super::PickSource { loc: 1, x: 0, z: 0 };
        assert_eq!(crate::kit::harvest::HarvestSource::unacked_rounds(&source), 1, "instant click, fast give-up");
        assert!(!crate::kit::harvest::HarvestSource::reprobe(&source, true, true, 999), "never re-clicks");
        assert!(EMPTY_LIMIT <= 5, "empty field ends fast: {EMPTY_LIMIT}");
    }
}
