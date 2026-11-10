//! Milking (P): use an empty bucket on a dairy cow, milk in the pack.
//! Declaration + NPC-harvest particularités on the kit: nearest cow,
//! one `watch_stand` click each (milking is instant, the milk is the
//! ack), banked loads through `kit::cycle`. No level, no station: cows
//! at Lumbridge and elsewhere (catalogue §6, P2).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bans::{BanSet, GONE_BAN_TICKS};
use crate::kit::bank::{bank_trip_resilient, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::harvest::{watch_stand, HarvestSource, HarvestTarget, WorkOutcome};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, is_walk_abort, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Cows the server milks (`cow_milking.rs2`: any other item refuses).
pub const COW_NAMES: [&str; 3] = ["cow", "cow2", "cow_beef"];
/// Empty bucket in, full bucket out (the server deletes the empty one).
pub const EMPTY: &str = "bucket_empty";
pub const PRODUCT: &str = "bucket_milk";
/// Start-of-work acks: none, the milk itself is the ack.
pub const MILK_ACKS: [&str; 0] = [];
/// Empty fields before the run ends loud instead of idling.
pub const EMPTY_LIMIT: u32 = 5;
/// Approach budget per cow.
pub const COW_WALK_TICKS: u64 = 6000;

/// One click, one cow: use the bucket, watch the milk land. Cows never
/// deplete (repeatable); a vanished or drifted slot ends the stand.
struct MilkSource {
    slot: u16,
    x: i32,
    z: i32,
}

impl HarvestSource for MilkSource {
    fn acks(&self) -> &[&str] {
        &MILK_ACKS
    }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<HarvestTarget, ScriptError> {
        let bucket = obj_id(bot, EMPTY)?;
        let (com, slot) = crate::quests::common::inv_slot(bot, bucket)?;
        bot.log(&format!("milking cow at ({x},{z})"));
        bot.intend("milking a cow");
        bot.op_npc_u(self.slot, bucket, u16::from(slot), com)?;
        Ok(HarvestTarget::Npc { slot: self.slot })
    }

    fn slot_gone(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        !bot.world.npcs.contains_key(&self.slot)
    }

    fn slot_moved(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        bot.world.npcs.get(&self.slot).is_some_and(|npc| (npc.x - self.x).abs() + (npc.z - self.z).abs() > 6)
    }

    fn target_live(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        bot.world.npcs.contains_key(&self.slot)
    }

    fn depleted_now(&self, _bot: &mut Bot, _x: i32, _z: i32, _silences: u32) -> bool {
        false
    }

    /// Instant action: one quiet round without milk means the click never
    /// landed — move on.
    fn unacked_rounds(&self) -> u32 {
        1
    }

    fn reprobe(&self, _acked: bool, _live: bool, _since_probe: u64) -> bool {
        false
    }
}

/// Buckets in the pack, topped up from the bank (cook pattern). None
/// anywhere blocks with the bucket name: the brain files the obtain task.
fn ensure_buckets(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, empty: u16) -> Result<bool, ScriptError> {
    if pack_total(bot, empty) > 0 {
        return Ok(true);
    }
    crate::kit::bank::bank_trip(bot, bank, &[], 0)?;
    let free = free_slots(bot) as u32;
    bank_withdraw_items(bot, bank, &[empty], free)?;
    Ok(pack_total(bot, empty) > 0)
}

/// Nearest cow outside the ban set, occupation-dodged.
fn pick_cow(bot: &mut Bot, banned: &BanSet, now: u64) -> Option<(u16, i32, i32)> {
    let mut cands: Vec<(u16, i32, i32)> = Vec::new();
    for (slot, x, z) in bot.npcs_named(&COW_NAMES) {
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

/// Work one cow: approach, click, watch the milk land.
fn work_cow(bot: &mut Bot, slot: u16, x: i32, z: i32, milk: u16) -> Result<(WorkOutcome, u32), ScriptError> {
    if let Err(error) = bot.approach_npc(slot, COW_WALK_TICKS) {
        let typed = ScriptError::from(error);
        if is_walk_abort(&typed) {
            return Err(typed);
        }
        return Ok((WorkOutcome::Unreachable, 0));
    }
    let had = pack_total(bot, milk);
    let mut source = MilkSource { slot, x, z };
    watch_stand(bot, &mut source, x, z, &[milk], had)
}

/// Milk banked loads in the open session: `milk [count]`.
pub fn run_milk(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("milk", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let empty = obj_id(bot, EMPTY)?;
    let milk = obj_id(bot, PRODUCT)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    if !ensure_buckets(bot, &bank, empty)? {
        return Err(ScriptError::Blocked { missing: vec![EMPTY.to_string()] });
    }
    let mut banned = BanSet::new();
    let mut empty_fields = 0u32;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        bank = nearest_bank(step_bot)?;
        if !ensure_buckets(step_bot, &bank, empty)? {
            return Err(ScriptError::Blocked { missing: vec![EMPTY.to_string()] });
        }
        let now = step_bot.world.ticks;
        let Some((slot, x, z)) = pick_cow(step_bot, &banned, now) else {
            empty_fields += 1;
            if empty_fields >= EMPTY_LIMIT {
                return Err(ScriptError::Usage(format!("no cows near {:?}", step_bot.position())));
            }
            step_bot.log("no cows, waiting");
            for _ in 0..10 {
                step_bot.pump(2)?;
            }
            return Ok(StepControl::Continue);
        };
        empty_fields = 0;
        let (outcome, _) = work_cow(step_bot, slot, x, z, milk)?;
        match outcome {
            WorkOutcome::Gone | WorkOutcome::Moved | WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot => {
                banned.ban_for((x, z), step_bot.world.ticks, GONE_BAN_TICKS);
                step_bot.log("cow gone, re-picking");
                return Ok(StepControl::Continue);
            }
            WorkOutcome::NoTool | WorkOutcome::NoBait => {
                return Err(ScriptError::Blocked { missing: vec![EMPTY.to_string()] });
            }
            WorkOutcome::Threat | WorkOutcome::Blocked | WorkOutcome::LevelLocked => {
                return Err(ScriptError::Usage(format!("milk stopped ({outcome:?}) at {:?}", step_bot.position())));
            }
            WorkOutcome::Full => {}
        }
        if pack_total(step_bot, empty) > 0 && free_slots(step_bot) > 0 {
            step_bot.log("cow milked, same load");
            return Ok(StepControl::Continue);
        }
        let keep: Vec<u16> = task_keep.clone();
        match bank_trip_resilient(step_bot, &mut bank, &keep, 0, &[milk]) {
            Ok(_) => {}
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died on the bank trip, restocking to continue");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
        }
        if !ensure_buckets(step_bot, &bank, empty)? {
            return Err(ScriptError::Blocked { missing: vec![EMPTY.to_string()] });
        }
        step_bot.log(&format!("ok load {} banked", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

#[cfg(test)]
mod tests {
    use super::{pick_cow, COW_NAMES, EMPTY, MILK_ACKS, PRODUCT};
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

    fn cow_type(bot: &Bot) -> u16 {
        (0u16..).find(|id| (bot.tables.npc_name)(*id).unwrap_or_default() == COW_NAMES[0]).expect("cow type")
    }

    /// Declaration: bucket in, milk out, the cow type exists, the click
    /// is instant (one quiet round, no re-click, no acks).
    #[test]
    fn declaration_matches_server_and_dump() {
        let bot = offline_bot();
        obj_id(&bot, EMPTY).expect("bucket_empty in tables");
        obj_id(&bot, PRODUCT).expect("bucket_milk in tables");
        assert!(MILK_ACKS.is_empty(), "the milk is the ack");
        let source = super::MilkSource { slot: 1, x: 0, z: 0 };
        assert_eq!(crate::kit::harvest::HarvestSource::unacked_rounds(&source), 1, "instant click, fast give-up");
        assert!(!crate::kit::harvest::HarvestSource::reprobe(&source, false, true, 999), "never re-clicks");
    }

    /// The dump carries cow spawns (the Lumbridge dairy herd); all three
    /// server-milked types resolve.
    #[test]
    fn dump_carries_cows() {
        let herd: usize = knowledge::all_npcs()
            .iter()
            .filter(|entry| COW_NAMES.contains(&entry.name.unwrap_or_default()))
            .map(|entry| entry.spawns.len())
            .sum();
        assert!(herd > 0, "cow spawns exist");
    }

    /// A milked cow stays milkable: liveness is presence, never a type
    /// flip (unlike sheep).
    #[test]
    fn liveness_is_presence() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let cow = cow_type(&bot);
        bot.world.npcs.insert(3u16, lifescape_sdk::world::NpcInfo { npc_type: cow, x: 10, z: 10, level: 0, name: None, anim: None });
        let source = super::MilkSource { slot: 3, x: 10, z: 10 };
        let target = crate::kit::harvest::HarvestTarget::Npc { slot: 3 };
        assert!(crate::kit::harvest::HarvestSource::target_live(&source, &mut bot, &target), "present cow is live");
        let gone = super::MilkSource { slot: 9, x: 0, z: 0 };
        let target = crate::kit::harvest::HarvestTarget::Npc { slot: 9 };
        assert!(!crate::kit::harvest::HarvestSource::target_live(&gone, &mut bot, &target), "vanished cow ends the stand");
        assert!(crate::kit::harvest::HarvestSource::slot_gone(&gone, &mut bot, &target), "vanished slot is gone");
    }

    /// Pick: nearest unbanned cow wins, banned ones are skipped.
    #[test]
    fn pick_skips_banned_cows() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let cow = cow_type(&bot);
        for (slot, x) in [(3u16, 10i32), (4, 30)] {
            bot.world.npcs.insert(slot, lifescape_sdk::world::NpcInfo { npc_type: cow, x, z: 0, level: 0, name: None, anim: None });
        }
        let banned = BanSet::new();
        assert_eq!(pick_cow(&mut bot, &banned, 0).map(|(_, x, _)| x), Some(10), "nearest wins");
        let mut banned = BanSet::new();
        banned.ban_for((10, 0), 0, 10_000);
        assert_eq!(pick_cow(&mut bot, &banned, 0).map(|(_, x, _)| x), Some(30), "banned skipped");
    }
}
