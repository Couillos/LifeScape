//! Essence run (R + service): Aubury teleports the bot to the rune essence
//! mine, the bot mines `blankrune` with a pickaxe and steps back through the
//! exit portal. Rune Mysteries must be complete or the teleport refuses
//! (loud, naming the quest). Talismans ride along kept, never banked: the
//! runecraft chain spends them at the altars. One F2P mine (4 rock spawns,
//! stations P2.1 via the rock dump, catalogue §5.2, P1, epoch P1).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;
use crate::kit::{bans::{BanSet, GONE_BAN_TICKS}, bank::{bank_trip_resilient, nearest_bank}, cycle::{parse_cli, resolve_keep, run, StepControl}};
use crate::kit::{harvest::{watch_stand, HarvestSource, HarvestTarget, WorkOutcome}, pack::{free_slots, obj_id, pack_total}, survival::{check_events, is_death_abort, is_walk_abort, wait_pack_streamed}, tools::{blocked_tool, recover_pickaxe, PICK_SPEC}};
use crate::{skill_walk::walk_checked, ScriptError, ScriptResult};

/// Catalogue §5.2 epoch and archetype (P1 essence mining, R).
pub const EPOCH: &str = "P1";
pub const ARCH: &str = "R";
/// Aubury's shop door tile and his teleport op (`aubury.rs2`: opnpc4).
pub const AUBURY: (i32, i32, i32) = (3253, 3402, 0);
pub const AUBURY_TELEPORT_OP: u8 = 4;
/// Essence rock, product and exit portal debugnames.
pub const ESSENCE_ROCK: &str = "blankrunestone";
pub const PRODUCT: &str = "blankrune";
pub const PORTAL: &str = "blankrunestone_exit_portal";
/// Runecraft talismans (F2P six): kept in the pack, never required here.
pub const TALISMANS: [&str; 6] =
    ["air_talisman", "mind_talisman", "water_talisman", "earth_talisman", "fire_talisman", "body_talisman"];
/// Quest gating the teleport (`essence_mine.rs2` refuses without it).
pub const NEED_QUEST: &str = "quest_rune_mysteries";
/// No live rock around before the trip ends (the mine never runs dry: the
/// rock pays without depleting, so an empty scan is a stream gap).
pub const EMPTY_LIMIT: u32 = 3;
/// Approach budget per rock and per Aubury walk.
pub const ESSENCE_WALK_TICKS: u64 = 6000;

/// True for a runecraft talisman debugname (kept, never banked).
pub fn is_talisman(name: &str) -> bool {
    TALISMANS.iter().any(|talisman| talisman.eq_ignore_ascii_case(name))
}

/// Loc id of the essence rock (pure dump scan, no service).
pub fn essence_loc_id(bot: &Bot) -> Option<u16> {
    (0u16..).find(|id| (bot.tables.loc_name)(*id).unwrap_or_default() == ESSENCE_ROCK)
}

/// Live essence rocks and portals around the bot (no service: the mine
/// streams its locs, static spawns cover the walk-in).
fn live_locs(bot: &Bot, name: &str, radius: i32) -> Vec<(i32, i32, u16)> {
    let mut found = Vec::new();
    for ((x, z, level), slot) in &bot.world.locs {
        let Some((loc, _, _)) = slot else { continue; };
        if *level == bot.world.level && (x - bot.world.x).abs() + (z - bot.world.z).abs() <= radius && (bot.tables.loc_name)(*loc).unwrap_or_default() == name {
            found.push((*x, *z, *loc));
        }
    }
    found.sort();
    found
}

/// Nearest live essence rock, occupation-dodged (the mine streams its locs;
/// a stream gap waits instead of guessing).
fn nearest_essence(bot: &mut Bot, banned: &BanSet, now: u64) -> Option<(i32, i32, u16)> {
    let mut cands: Vec<(i64, (i32, i32, u16))> = Vec::new();
    for (x, z, loc) in live_locs(bot, ESSENCE_ROCK, 60) {
        if banned.is_banned((x, z), now) {
            continue;
        }
        cands.push((i64::from((x - bot.world.x).abs() + (z - bot.world.z).abs()), (x, z, loc)));
    }
    let tiles: Vec<(i64, (i32, i32))> = cands.iter().map(|(dist, (x, z, _))| (*dist, (*x, *z))).collect();
    let free = crate::kit::spots::drop_occupied(bot, tiles, crate::kit::spots::OCCUPIED_RADIUS);
    free.into_iter()
        .filter_map(|(_, (x, z))| cands.iter().find(|(_, tile)| (tile.0, tile.1) == (x, z)).copied())
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, tile)| tile)
}

/// One click, one rock: the mine runs on the mining script (`mining.rs2`
/// `blankrunestone` arms), so the acks and the re-click probe are mining's.
struct EssenceSource {
    loc: u16,
    x: i32,
    z: i32,
}

impl HarvestSource for EssenceSource {
    fn acks(&self) -> &[&str] { &crate::kit::mine::MINING_ACKS }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<HarvestTarget, ScriptError> {
        bot.log(&format!("mining essence at ({x},{z})"));
        bot.intend("mining rune essence");
        bot.op_loc(1, x, z, self.loc)?;
        Ok(HarvestTarget::Loc { loc: self.loc })
    }

    fn slot_gone(&self, _bot: &mut Bot, _target: &HarvestTarget) -> bool { false }
    fn slot_moved(&self, _bot: &mut Bot, _target: &HarvestTarget) -> bool { false }
    fn target_live(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool { bot.world.loc_at(self.x, self.z, bot.world.level).is_none_or(|(id, _, _)| id == self.loc) }
    fn depleted_now(&self, _bot: &mut Bot, _x: i32, _z: i32, _silences: u32) -> bool { false }
    /// Mining probes reachability after every quiet round (a wall-separated
    /// rock only refuses when clicked).
    fn reprobe(&self, _acked: bool, _live: bool, _since_probe: u64) -> bool { true }
}

/// Work one rock: walk over (blocked décor settles adjacent, Ok), watch the
/// essence land.
fn work_essence(bot: &mut Bot, x: i32, z: i32, loc: u16, essence: u16) -> Result<(WorkOutcome, u32), ScriptError> {
    match walk_checked(bot, x, z, ESSENCE_WALK_TICKS) {
        Ok(true) => {}
        Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
        Err(error) if is_walk_abort(&error) => return Err(error),
        Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
    }
    let had = pack_total(bot, essence);
    let mut source = EssenceSource { loc, x, z };
    watch_stand(bot, &mut source, x, z, &[essence], had)
}

/// Aubury's teleport to the mine (op 4, no dialogue). Skipped when essence
/// rock already streams nearby. A quest refusal fails loud naming the quest.
fn teleport_to_mine(bot: &mut Bot) -> Result<(), ScriptError> {
    if !live_locs(bot, ESSENCE_ROCK, 60).is_empty() {
        return Ok(());
    }
    let slot = bot.find_npc("aubury").ok_or_else(|| ScriptError::Usage(format!("no Aubury near {:?}", bot.position())))?;
    bot.approach_npc(slot, ESSENCE_WALK_TICKS)?;
    bot.log("asking Aubury for the essence mine");
    bot.intend("teleporting to the rune essence mine");
    bot.op_npc(AUBURY_TELEPORT_OP, slot)?;
    let target = bot.world.ticks + 100;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if !live_locs(bot, ESSENCE_ROCK, 60).is_empty() {
            bot.log("arrived in the essence mine");
            return Ok(());
        }
    }
    if crate::quests::common::has_message(bot, "rune mysteries") {
        return Err(ScriptError::Usage(format!("essence needs {NEED_QUEST} at {:?}", bot.position())));
    }
    Err(ScriptError::Usage(format!("Aubury did not teleport at {:?}", bot.position())))
}

/// Step back through the exit portal (op 1, `essence_mine.rs2` picks the
/// return tile from the entry side).
fn return_through_portal(bot: &mut Bot) -> Result<(), ScriptError> {
    let target = bot.world.ticks + 200;
    while bot.world.ticks < target {
        if let Some((x, z, loc)) = live_locs(bot, PORTAL, 60).into_iter().next() {
            if !walk_checked(bot, x, z, ESSENCE_WALK_TICKS)? { return Err(ScriptError::Usage(format!("portal unreachable at ({x},{z})"))); }
            bot.op_loc(1, x, z, loc)?;
            let back = bot.world.ticks + 100;
            while bot.world.ticks < back {
                bot.pump(2)?;
                if live_locs(bot, ESSENCE_ROCK, 60).is_empty() {
                    bot.log("back from the essence mine");
                    return Ok(());
                }
            }
            return Err(ScriptError::Usage(format!("portal did not move us at {:?}", bot.position())));
        }
        bot.pump(5)?;
    }
    Err(ScriptError::Usage(format!("no essence portal near {:?}", bot.position())))
}

/// Talismans and tools the bank trip keeps (the runecraft chain spends the
/// talismans at the altars, never banks them).
fn keep_ids(bot: &Bot, task_keep: &[u16]) -> Vec<u16> {
    let mut keep: Vec<u16> = PICK_SPEC.keep_ids(bot);
    keep.extend(task_keep.iter().copied());
    for name in TALISMANS {
        if let Ok(id) = obj_id(bot, name) {
            if pack_total(bot, id) > 0 && !keep.contains(&id) {
                keep.push(id);
            }
        }
    }
    keep
}

/// Mine banked loads through Aubury in the open session: `essence_run [count]`.
pub fn run_essence_run(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = parse_cli("essence_run", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let essence = obj_id(bot, PRODUCT)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    if PICK_SPEC.best_held(bot).is_none() && !recover_pickaxe(bot, coins, &bank)? {
        return Err(blocked_tool("bronze_pickaxe"));
    }
    let mut banned = BanSet::new();
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if PICK_SPEC.best_held(step_bot).is_none() && !recover_pickaxe(step_bot, coins, &bank)? {
            return Err(blocked_tool("bronze_pickaxe"));
        }
        teleport_to_mine(step_bot)?;
        let mut empty = 0u32;
        while free_slots(step_bot) > 0 {
            let now = step_bot.world.ticks;
            let Some((x, z, loc)) = nearest_essence(step_bot, &banned, now) else {
                empty += 1;
                if empty >= EMPTY_LIMIT {
                    break;
                }
                step_bot.pump(10)?;
                continue;
            };
            empty = 0;
            let (outcome, _) = work_essence(step_bot, x, z, loc, essence)?;
            match outcome {
                WorkOutcome::Gone | WorkOutcome::Moved | WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot => {
                    banned.ban_for((x, z), step_bot.world.ticks, GONE_BAN_TICKS);
                }
                WorkOutcome::NoTool => return Err(blocked_tool("bronze_pickaxe")),
                _ => break,
            }
        }
        return_through_portal(step_bot)?;
        bank = nearest_bank(step_bot)?;
        let trip_keep = keep_ids(step_bot, &task_keep);
        match bank_trip_resilient(step_bot, &mut bank, &trip_keep, 0, &[essence]) {
            Ok(_) => {}
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died on the bank trip, mining again");
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
    use super::{essence_loc_id, is_talisman, live_locs, nearest_essence, ARCH, AUBURY, AUBURY_TELEPORT_OP, EMPTY_LIMIT, EPOCH, ESSENCE_ROCK, NEED_QUEST, PORTAL, PRODUCT, TALISMANS};
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

    /// Declaration: epoch P1, archetype R, essence rock/product/portal and
    /// all six talismans resolve F2P; the rock mines at level 1 for 5 XP.
    #[test]
    fn declaration_matches_server_and_dump() {
        assert_eq!((EPOCH, ARCH), ("P1", "R"));
        let bot = offline_bot();
        obj_id(&bot, PRODUCT).expect("essence in tables");
        let row = knowledge::objs_by_name(PRODUCT).into_iter().next().expect("dump row");
        assert_eq!(row.members, Some(false), "essence is F2P, got {:?}", row.members);
        essence_loc_id(&bot).expect("essence loc resolves through the tables");
        for name in TALISMANS {
            obj_id(&bot, name).expect("talisman in tables");
            assert!(is_talisman(name), "{name} keeps");
            let row = knowledge::objs_by_name(name).into_iter().next().expect("dump row");
            assert_eq!(row.members, Some(false), "{name} is F2P, got {:?}", row.members);
        }
        assert!(!is_talisman("coins"), "junk never keeps");
        assert!(!is_talisman("law_talisman"), "members talisman never keeps");
        let tier = knowledge::rocks::all_rock_tiers().into_iter().find(|tier| tier.rocks.iter().any(|rock| rock == ESSENCE_ROCK)).expect("essence tier");
        assert_eq!((tier.level, tier.product.as_str(), tier.exp), (1, PRODUCT, 50), "mine 1, 50 tenths, got {tier:?}");
        assert_eq!(tier.product_members, Some(false), "F2P product");
        assert_eq!((AUBURY_TELEPORT_OP, NEED_QUEST), (4, "quest_rune_mysteries"));
    }

    /// Aubury stands in Varrock F2P (service + NPC ops carry Teleport) and
    /// the exit portal exists in the dump.
    #[test]
    fn aubury_and_portal_match_services() {
        let aubury = knowledge::services::all_services().into_iter().find(|entry| entry.key == "aubury_teleport").expect("aubury service");
        assert_eq!((aubury.x, aubury.z), (AUBURY.0 as i64, AUBURY.1 as i64), "Varrock shop, got ({},{})", aubury.x, aubury.z);
        let npc = knowledge::all_npcs().into_iter().find(|entry| entry.name.unwrap_or_default() == "aubury").expect("aubury npc");
        assert!(npc.ops.iter().flatten().any(|op| *op == "Teleport"), "teleport op, got {:?}", npc.ops);
        assert!(knowledge::all_locs().iter().any(|entry| entry.name.as_deref() == Some(PORTAL)), "exit portal in dumps");
    }

    /// Live scan: a streamed essence rock resolves by exact debugname, other
    /// locs never do; nearest skips banned rocks.
    #[test]
    fn live_scan_finds_unbanned_rocks() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 2900;
        bot.world.z = 4830;
        let loc = essence_loc_id(&bot).expect("essence loc");
        bot.world.locs.insert((2901, 4831, 0), Some((loc, 0, 0)));
        bot.world.locs.insert((2902, 4832, 0), Some((loc.wrapping_add(1), 0, 0)));
        let found = live_locs(&bot, ESSENCE_ROCK, 60);
        assert_eq!(found, vec![(2901, 4831, loc)], "exact debugname only, got {found:?}");
        let banned = BanSet::new();
        assert_eq!(nearest_essence(&mut bot, &banned, 0), Some((2901, 4831, loc)), "live rock picks");
        let mut banned = BanSet::new();
        banned.ban_for((2901, 4831), 0, 10_000);
        assert!(nearest_essence(&mut bot, &banned, 0).is_none(), "banned rock skipped, gap waits");
    }

    /// Bounds: the mine never depletes (the rock pays without `loc_change`),
    /// so an empty scan ends the trip after a few waits, never idles.
    #[test]
    fn empty_scan_ends_fast() {
        assert!(EMPTY_LIMIT <= 5, "empty mine ends fast: {EMPTY_LIMIT}");
        assert_eq!(crate::kit::mine::MINING_ACKS, ["swing your pick"], "mining acks shared");
    }
}
