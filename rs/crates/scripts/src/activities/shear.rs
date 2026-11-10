//! Shearing (P): use shears on an unsheared sheep, wool in the pack.
//! Declaration + NPC-harvest particularités on the kit: nearest live
//! sheep, one `watch_stand` click each (shearing is instant, the wool is
//! the ack), banked loads through `kit::cycle`. No level, no station:
//! 27 F2P sheep at Lumbridge (catalogue §6, P1, Sheep Shearer).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bans::{BanSet, GONE_BAN_TICKS};
use crate::kit::bank::{bank_trip_resilient, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::harvest::{watch_stand, HarvestSource, HarvestTarget, WorkOutcome};
use crate::kit::pack::{free_slots, obj_id, obj_price, pack_total};
use crate::kit::shop::buy_stock;
use crate::kit::survival::{check_events, is_death_abort, is_walk_abort, wait_pack_streamed};
use crate::kit::tools::{blocked_tool, ToolSpec};
use crate::{ScriptError, ScriptResult};

/// Sheep the server shears (`shear_sheep.rs2`: any other item refuses).
pub const SHEEP_NAMES: [&str; 1] = ["sheepunsheered"];
/// Tool and product debug names.
pub const TOOL: &str = "shears";
pub const PRODUCT: &str = "wool";
/// Single-tool rank (no handle/head: shears never split).
pub const SHEAR_SPEC: ToolSpec = ToolSpec { order: &["shears"] };
/// Start-of-work acks: none, the wool itself is the ack.
pub const SHEAR_ACKS: [&str; 0] = [];
/// Sheardless loads before the run reports blocked (the brain files the
/// obtain task from `missing`, no tool hunting here).
pub const TOOLLESS_LIMIT: u32 = 3;
/// Empty fields before the run ends loud instead of idling.
pub const EMPTY_LIMIT: u32 = 5;
/// Approach budget per sheep.
pub const SHEEP_WALK_TICKS: u64 = 6000;

/// One click, one sheep: use the shears, watch the wool land. A sheared
/// sheep is depletion (the type flips to `sheepsheered`); a fled sheep
/// reads as moved. Never re-clicks: the action is instant.
struct ShearSource {
    slot: u16,
    x: i32,
    z: i32,
}

impl HarvestSource for ShearSource {
    fn acks(&self) -> &[&str] {
        &SHEAR_ACKS
    }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<HarvestTarget, ScriptError> {
        let tool = obj_id(bot, TOOL)?;
        let (com, slot) = crate::quests::common::inv_slot(bot, tool)?;
        bot.log(&format!("shearing sheep at ({x},{z})"));
        bot.intend("shearing a sheep");
        bot.op_npc_u(self.slot, tool, u16::from(slot), com)?;
        Ok(HarvestTarget::Npc { slot: self.slot })
    }

    fn slot_gone(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        !bot.world.npcs.contains_key(&self.slot)
    }

    fn slot_moved(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        bot.world.npcs.get(&self.slot).is_some_and(|npc| (npc.x - self.x).abs() + (npc.z - self.z).abs() > 6)
    }

    fn target_live(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        bot.world.npcs.get(&self.slot).is_some_and(|npc| {
            (bot.tables.npc_name)(npc.npc_type).unwrap_or_default() == SHEEP_NAMES[0]
        })
    }

    fn depleted_now(&self, _bot: &mut Bot, _x: i32, _z: i32, _silences: u32) -> bool {
        false
    }

    /// Instant action: one quiet round without wool means the click never
    /// landed (fled sheep, race lost) — move on.
    fn unacked_rounds(&self) -> u32 {
        1
    }

    fn reprobe(&self, _acked: bool, _live: bool, _since_probe: u64) -> bool {
        false
    }
}

/// Free shears first (pack, banked), then the 1-coin shop buy; anything
/// else blocks with the buyable name.
fn ensure_shears(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, coins: u16) -> Result<bool, ScriptError> {
    if SHEAR_SPEC.best_held(bot).is_some() {
        return Ok(true);
    }
    let mut keep = vec![coins];
    keep.extend(SHEAR_SPEC.known_ids(bot));
    crate::kit::bank::bank_trip(bot, bank, &keep, 0)?;
    if SHEAR_SPEC.best_held(bot).is_none() && crate::kit::tools::restore_banked_tool(bot, bank, &SHEAR_SPEC)? {
        bot.log("ok shears restored from bank");
    }
    if SHEAR_SPEC.best_held(bot).is_some() {
        return Ok(true);
    }
    let price = obj_price(TOOL) as u32;
    if pack_total(bot, coins) >= price && buy_stock(bot, TOOL, 1)? > 0 {
        bot.log("ok bootstrapped shears");
        return Ok(true);
    }
    Ok(false)
}

/// Nearest unsheared sheep outside the ban set, occupation-dodged.
fn pick_sheep(bot: &mut Bot, banned: &BanSet, now: u64) -> Option<(u16, i32, i32)> {
    let mut cands: Vec<(u16, i32, i32)> = Vec::new();
    for (slot, x, z) in bot.npcs_named(&SHEEP_NAMES) {
        if banned.is_banned((x, z), now) {
            continue;
        }
        if (x - bot.world.x).abs() + (z - bot.world.z).abs() > 400 {
            continue;
        }
        cands.push((slot, x, z));
    }
    let tiles: Vec<(i64, (i32, i32))> =
        cands.iter().map(|(_, x, z)| (i64::from((x - bot.world.x).abs() + (z - bot.world.z).abs()), (*x, *z))).collect();
    let free = crate::kit::spots::drop_occupied(bot, tiles, crate::kit::spots::OCCUPIED_RADIUS);
    free.into_iter()
        .filter_map(|(_, (x, z))| cands.iter().find(|(_, cx, cz)| (*cx, *cz) == (x, z)).copied())
        .min_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs())
}

/// Work one sheep: approach, click, watch the wool land.
fn work_sheep(bot: &mut Bot, slot: u16, x: i32, z: i32, wool: u16) -> Result<(WorkOutcome, u32), ScriptError> {
    if let Err(error) = bot.approach_npc(slot, SHEEP_WALK_TICKS) {
        let typed = ScriptError::from(error);
        if is_walk_abort(&typed) {
            return Err(typed);
        }
        return Ok((WorkOutcome::Unreachable, 0));
    }
    let had = pack_total(bot, wool);
    let mut source = ShearSource { slot, x, z };
    watch_stand(bot, &mut source, x, z, &[wool], had)
}

/// Shear banked loads in the open session: `shear [count]`.
pub fn run_shear(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("shear", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let wool = obj_id(bot, PRODUCT)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    if !ensure_shears(bot, &bank, coins)? {
        return Err(blocked_tool(TOOL));
    }
    let mut banned = BanSet::new();
    let mut toolless = 0u32;
    let mut empty = 0u32;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        bank = nearest_bank(step_bot)?;
        if SHEAR_SPEC.best_held(step_bot).is_none() {
            if ensure_shears(step_bot, &bank, coins)? {
                toolless = 0;
                return Ok(StepControl::Continue);
            }
            toolless += 1;
            if toolless >= TOOLLESS_LIMIT {
                return Err(blocked_tool(TOOL));
            }
            step_bot.log("still sheardless, retrying re-gear next load");
            return Ok(StepControl::Continue);
        }
        toolless = 0;
        let now = step_bot.world.ticks;
        let Some((slot, x, z)) = pick_sheep(step_bot, &banned, now) else {
            empty += 1;
            if empty >= EMPTY_LIMIT {
                return Err(ScriptError::Usage(format!("no sheep near {:?}", step_bot.position())));
            }
            step_bot.log("no sheep, waiting for respawns");
            for _ in 0..10 {
                step_bot.pump(2)?;
            }
            return Ok(StepControl::Continue);
        };
        empty = 0;
        let (outcome, _) = work_sheep(step_bot, slot, x, z, wool)?;
        match outcome {
            WorkOutcome::Gone | WorkOutcome::Moved | WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot => {
                banned.ban_for((x, z), step_bot.world.ticks, GONE_BAN_TICKS);
                step_bot.log("sheep gone, re-picking");
                return Ok(StepControl::Continue);
            }
            WorkOutcome::NoTool => {
                toolless += 1;
                if toolless >= TOOLLESS_LIMIT {
                    return Err(blocked_tool(TOOL));
                }
                return Ok(StepControl::Continue);
            }
            WorkOutcome::Threat | WorkOutcome::Blocked | WorkOutcome::LevelLocked => {
                return Err(ScriptError::Usage(format!("shear stopped ({outcome:?}) at {:?}", step_bot.position())));
            }
            WorkOutcome::Full | WorkOutcome::NoBait => {}
        }
        if free_slots(step_bot) > 0 {
            step_bot.log("sheep shorn, same load");
            return Ok(StepControl::Continue);
        }
        let mut keep: Vec<u16> = SHEAR_SPEC.keep_ids(step_bot);
        keep.extend(task_keep.iter().copied());
        match bank_trip_resilient(step_bot, &mut bank, &keep, 0, &[wool]) {
            Ok(_) => {}
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died on the bank trip, re-gearing to continue");
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
    use super::{pick_sheep, PRODUCT, SHEAR_ACKS, SHEAR_SPEC, SHEEP_NAMES, TOOL, TOOLLESS_LIMIT};
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

    fn sheep_type(bot: &Bot) -> u16 {
        (0u16..).find(|id| (bot.tables.npc_name)(*id).unwrap_or_default() == SHEEP_NAMES[0]).expect("sheepunsheered type")
    }

    /// Declaration: tool and product resolve, the NPC type exists, the
    /// click is instant (one quiet round, no re-click, no acks).
    #[test]
    fn declaration_matches_server_and_dump() {
        let bot = offline_bot();
        obj_id(&bot, TOOL).expect("shears in tables");
        obj_id(&bot, PRODUCT).expect("wool in tables");
        assert_eq!(SHEAR_SPEC.best_held(&bot), None, "empty pack holds no shears");
        assert!(SHEAR_ACKS.is_empty(), "the wool is the ack");
        let source = super::ShearSource { slot: 1, x: 0, z: 0 };
        assert_eq!(crate::kit::harvest::HarvestSource::unacked_rounds(&source), 1, "instant click, fast give-up");
        assert!(!crate::kit::harvest::HarvestSource::reprobe(&source, true, true, 999), "never re-clicks");
        assert!(TOOLLESS_LIMIT <= 5, "no tool-hunting loops: {TOOLLESS_LIMIT}");
    }

    /// The dump carries unsheared sheep spawns (the Lumbridge flock the
    /// catalogue counts); sheared ones are depletion, never picks.
    #[test]
    fn dump_carries_unsheared_sheep() {
        let unsheared: usize = knowledge::all_npcs()
            .iter()
            .filter(|entry| entry.name.unwrap_or_default() == SHEEP_NAMES[0])
            .map(|entry| entry.spawns.len())
            .sum();
        assert!(unsheared > 0, "unsheared spawns exist");
    }

    /// Target liveness follows the type flip: unsheered is live, a
    /// sheared (or vanished) slot ends the stand.
    #[test]
    fn liveness_follows_the_type_flip() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let sheep = sheep_type(&bot);
        let other = sheep.wrapping_add(1);
        bot.world.npcs.insert(3u16, lifescape_sdk::world::NpcInfo { npc_type: sheep, x: 10, z: 10, level: 0, name: None, anim: None });
        bot.world.npcs.insert(4u16, lifescape_sdk::world::NpcInfo { npc_type: other, x: 12, z: 10, level: 0, name: None, anim: None });
        let live = super::ShearSource { slot: 3, x: 10, z: 10 };
        let target = crate::kit::harvest::HarvestTarget::Npc { slot: 3 };
        assert!(crate::kit::harvest::HarvestSource::target_live(&live, &mut bot, &target), "unsheered is live");
        assert!(!crate::kit::harvest::HarvestSource::slot_gone(&live, &mut bot, &target), "present slot is not gone");
        let shorn = super::ShearSource { slot: 4, x: 12, z: 10 };
        let target = crate::kit::harvest::HarvestTarget::Npc { slot: 4 };
        assert!(!crate::kit::harvest::HarvestSource::target_live(&shorn, &mut bot, &target), "sheared ends the stand");
        let gone = super::ShearSource { slot: 9, x: 0, z: 0 };
        let target = crate::kit::harvest::HarvestTarget::Npc { slot: 9 };
        assert!(crate::kit::harvest::HarvestSource::slot_gone(&gone, &mut bot, &target), "vanished slot is gone");
    }

    /// Pick: nearest unbanned sheep wins, banned ones are skipped.
    #[test]
    fn pick_skips_banned_sheep() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let sheep = sheep_type(&bot);
        for (slot, x) in [(3u16, 10i32), (4, 30)] {
            bot.world.npcs.insert(slot, lifescape_sdk::world::NpcInfo { npc_type: sheep, x, z: 0, level: 0, name: None, anim: None });
        }
        let banned = BanSet::new();
        assert_eq!(pick_sheep(&mut bot, &banned, 0).map(|(_, x, _)| x), Some(10), "nearest wins");
        let mut banned = BanSet::new();
        banned.ban_for((10, 0), 0, 10_000);
        assert_eq!(pick_sheep(&mut bot, &banned, 0).map(|(_, x, _)| x), Some(30), "banned skipped");
    }
}
