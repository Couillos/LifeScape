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
pub use crate::kit::essence_mine::{in_mine, live_locs, mine_locs, return_through_portal, teleport_to_mine, MINE_SQUARE};

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
/// Walk budget to one orthogonal side of the exit portal.
pub const PORTAL_STAND_TICKS: u64 = 1500;

/// True for a runecraft talisman debugname (kept, never banked).
pub fn is_talisman(name: &str) -> bool {
    TALISMANS.iter().any(|talisman| talisman.eq_ignore_ascii_case(name))
}

/// Loc id of the essence rock (pure dump scan, no service).
pub fn essence_loc_id(bot: &Bot) -> Option<u16> {
    (0u16..).find(|id| (bot.tables.loc_name)(*id).unwrap_or_default() == ESSENCE_ROCK)
}

/// Nearest essence rock among `rocks`, occupation-dodged.
fn nearest_essence(bot: &mut Bot, rocks: Vec<(i32, i32, u16)>, banned: &BanSet, now: u64) -> Option<(i32, i32, u16)> {
    let mut cands: Vec<(i64, (i32, i32, u16))> = Vec::new();
    for (x, z, loc) in rocks {
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

/// A rock reads live while the bot stands in the mine and the rock is
/// either unstreamed there (static locs never stream) or still the rock.
fn rock_live(x: i32, z: i32, level: i32, seen: Option<u16>, rock: u16) -> bool {
    crate::kit::essence_mine::in_mine(x, z, level) && seen.is_none_or(|id| id == rock)
}

/// One click, one rock: the mine runs on the mining script (`mining.rs2`
/// `blankrunestone` arms: `get_ore_essence` chains with `p_oploc(3)`,
/// the rock never depletes), so the acks are mining's and the stand
/// waits like woodcut: no re-click while the swing is acked (live
/// 2026-11-16: 52 319 re-clicks every 12 ticks).
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
    /// Live only inside the mine with the rock in view: out of the mine
    /// (a market delivery walked the bot to Varrock) the stand ends
    /// (live 2026-11-15, alice_knight clicked the rock from the Varrock
    /// bank 47,875 times over 10 h).
    fn target_live(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool { rock_live(bot.world.x, bot.world.z, bot.world.level, bot.world.loc_at(self.x, self.z, bot.world.level).map(|(id, _, _)| id), self.loc) }
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
            // Pulled out mid-load (market delivery, rescue): back in
            // through Aubury before any rock.
            teleport_to_mine(step_bot)?;
            let now = step_bot.world.ticks;
            let rocks = mine_locs(step_bot, ESSENCE_ROCK, 60)?;
            let Some((x, z, loc)) = nearest_essence(step_bot, rocks, &banned, now) else {
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
        Ok(state.load_done(step_bot, "banked"))
    })
}

#[cfg(test)]
mod tests {
    /// Out of the mine no rock is live, whatever streams there; inside,
    /// an unstreamed static rock still is.
    #[test]
    fn a_rock_is_never_live_from_outside_the_mine() {
        let rock = 2491;
        assert!(!super::rock_live(3185, 3436, 0, None, rock), "Varrock bank: the stand ends");
        assert!(super::rock_live(2911, 4832, 0, None, rock), "in the mine, unstreamed static rock");
        assert!(!super::rock_live(2911, 4832, 0, Some(1), rock), "another loc there");
    }

    use super::{essence_loc_id, in_mine, is_talisman, live_locs, nearest_essence, ARCH, AUBURY, AUBURY_TELEPORT_OP, EMPTY_LIMIT, EPOCH, ESSENCE_ROCK, NEED_QUEST, PORTAL, PRODUCT, TALISMANS};
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
        assert_eq!(nearest_essence(&mut bot, found.clone(), &banned, 0), Some((2901, 4831, loc)), "live rock picks");
        let mut banned = BanSet::new();
        banned.ban_for((2901, 4831), 0, 10_000);
        assert!(nearest_essence(&mut bot, found, &banned, 0).is_none(), "banned rock skipped, gap waits");
    }

    /// Live 2026-11-13 (62 "no Aubury near" over 10 bots, mining and
    /// runecraft banned): the run looked for Aubury from Lumbridge, Draynor
    /// or Al Kharid and failed at once. Out of view, the run walks to his
    /// shop (spawn `m50_53` 53 10 = 3253,3402,0, aubury id 553) first; in
    /// view, it talks to him where he stands.
    #[test]
    fn aubury_out_of_view_walks_to_his_shop() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 3232;
        bot.world.z = 3203;
        assert_eq!(crate::kit::shop::spawn_walk(&bot, "aubury", (AUBURY.0, AUBURY.1)), Some((AUBURY.0, AUBURY.1)), "Lumbridge: walk to Varrock first");
        let aubury = (0u16..4000).find(|id| (bot.tables.npc_name)(*id).unwrap_or_default() == "aubury").expect("aubury npc id");
        bot.world.x = 3250;
        bot.world.z = 3400;
        bot.world.npcs.insert(7u16, lifescape_sdk::world::NpcInfo { npc_type: aubury, x: 3253, z: 3402, level: 0, name: None, anim: None, face_entity: None, hit_tick: None });
        assert_eq!(crate::kit::shop::spawn_walk(&bot, "aubury", (AUBURY.0, AUBURY.1)), None, "streamed: no walk");
    }

    /// The mine is map square 45_75 (`m45_75.jm2`: rocks 2491 at
    /// (2891,4847) (2893,4812) (2925,4848) (2927,4814), portals 2492 at
    /// (2885,4850) (2889,4813) (2932,4854) (2933,4815)); t48 landed at
    /// (2912,4825). Varrock, the air temple (44_75) and level 1 are out.
    #[test]
    fn mine_square_bounds() {
        for (x, z) in [(2912, 4825), (2891, 4847), (2927, 4814), (2885, 4850), (2933, 4815)] {
            assert!(in_mine(x, z, 0), "({x},{z}) in the mine");
        }
        assert!(!in_mine(3253, 3402, 0), "Aubury's shop");
        assert!(!in_mine(2841, 4828, 0), "air temple");
        assert!(!in_mine(2912, 4825, 1), "another level");
    }

    /// Bounds: the mine never depletes (the rock pays without `loc_change`),
    /// so an empty scan ends the trip after a few waits, never idles.
    #[test]
    fn empty_scan_ends_fast() {
        assert!(EMPTY_LIMIT <= 5, "empty mine ends fast: {EMPTY_LIMIT}");
        assert_eq!(crate::kit::mine::MINING_ACKS, ["swing your pick"], "mining acks shared");
    }
}
