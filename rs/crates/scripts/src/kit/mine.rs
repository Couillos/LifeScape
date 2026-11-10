//! Mine driver: the rock work loop (LoopOps) on the shared harvest
//! core, pickaxe recovery and banked restores. The pool (tiers, spawns,
//! liveness) lives in `kit::spots`; the activity keeps only the CLI and
//! the loads loop.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::spots::{pick_rock_spot, pick_rock_tier, rock_live_loc_id, rock_still_there};
use crate::kit::tools::{recover_pickaxe, PICK_SPEC};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::harvest::WorkOutcome;
use crate::kit::bans::GONE_BAN_TICKS;
use crate::kit::survival::learn_level;
use crate::skill_walk::walk_checked;
use crate::ScriptError;

pub const MINING_STAT: u8 = 14;

/// Server start-of-work ack: the action loop runs on the server's side.
/// Mining-only vocabulary (point 1: skill-specific stays with the skill).
pub const MINING_ACKS: [&str; 1] = ["swing your pick"];

/// Click once, then only re-click after a quiet round (the server chains
/// attempts itself while in range). Rock liveness is the loc id match:
/// depletion streams a loc change, so an acked swing on a still-matching
/// rock is a dry streak — keep waiting. Quiet rounds re-click as
/// reachability probes: only a click surfaces a refusal.
struct RockSource {
    loc: u16,
    x: i32,
    z: i32,
}

impl crate::kit::harvest::HarvestSource for RockSource {
    fn acks(&self) -> &[&str] {
        &MINING_ACKS
    }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<crate::kit::harvest::HarvestTarget, ScriptError> {
        bot.log(&format!("click rock at ({x},{z})"));
        bot.intend(&format!("mining a rock at ({x},{z})"));
        bot.op_loc(1, x, z, self.loc)?;
        Ok(crate::kit::harvest::HarvestTarget::Loc { loc: self.loc })
    }

    fn slot_gone(&self, _bot: &mut Bot, _target: &crate::kit::harvest::HarvestTarget) -> bool {
        false
    }

    fn slot_moved(&self, _bot: &mut Bot, _target: &crate::kit::harvest::HarvestTarget) -> bool {
        false
    }

    fn target_live(&self, bot: &mut Bot, _target: &crate::kit::harvest::HarvestTarget) -> bool {
        rock_live_loc_id(bot, self.x, self.z).is_ok_and(|id| id == self.loc)
    }

    fn depleted_now(&self, _bot: &mut Bot, _x: i32, _z: i32, _silences: u32) -> bool {
        false
    }

    /// Quiet rounds re-click as reachability probes: a wall-separated
    /// rock only refuses when clicked.
    fn reprobe(&self, _acked: bool, _live: bool, _since_probe: u64) -> bool {
        true
    }

    fn note_dry_streak(&mut self, bot: &mut Bot, x: i32, z: i32, acked: bool, live: bool, silences: u32) {
        if silences == 2 && acked && live {
            bot.log(&format!("still mining at ({x},{z}), waiting out the dry streak"));
        }
    }
}

fn work_spot(
    bot: &mut Bot,
    rock_x: i32,
    rock_z: i32,
    ore_ids: &[u16],
    had_ore: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable rock: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, rock_x, rock_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::kit::survival::is_walk_abort(&error) => return Err(error),
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 1 {
        if !crate::skill_walk::step_adjacent(bot, rock_x, rock_z)? {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let rock_loc = rock_live_loc_id(bot, rock_x, rock_z)?;
    let mut source = RockSource { loc: rock_loc, x: rock_x, z: rock_z };
    crate::kit::harvest::watch_stand(bot, &mut source, rock_x, rock_z, ore_ids, had_ore)
}

/// Mining policy on the shared driver: full pack ends the load, a lost
/// pickaxe exits for caller-side recovery (reattach, shop, bank), tier
/// upgrades and dead rocks drop the spot.
struct MineOps<'a> {
    tiers: &'a [knowledge::rocks::RockTier],
    bank: &'a knowledge::banks::Bank,
    coins: u16,
    ore_ids: Vec<u16>,
    level: i64,
}

impl crate::kit::load::LoopOps for MineOps<'_> {
    type Spot = knowledge::rocks::RockTier;

    fn is_full(&self, bot: &Bot) -> bool {
        free_slots(bot) <= usize::from(pack_total(bot, self.coins) > 0)
    }

    fn has_tool(&self, bot: &Bot) -> bool {
        PICK_SPEC.best_held(bot).is_some()
    }

    fn gain_of(&self, bot: &Bot, _spot: &Self::Spot) -> u32 {
        self.ore_ids.iter().map(|id| pack_total(bot, *id)).sum()
    }

    fn pick(&mut self, bot: &mut Bot, state: &mut crate::kit::load::LoopState) -> Option<(Self::Spot, i32, i32)> {
        self.level = learn_level(bot, MINING_STAT, self.level);
        let now = bot.world.ticks;
        let found = pick_rock_spot(bot, self.tiers, self.bank, self.level, &state.banned, now);
        match found {
            Some((ref tier, spot_x, spot_z)) => {
                bot.log(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
                bot.intend(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
            }
            None => {
                bot.log("no rock spot, waiting for respawns");
                bot.intend("waiting for a safe rock spot");
            }
        }
        found
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::kit::load::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
    ) -> bool {
        self.level = learn_level(bot, MINING_STAT, self.level);
        if let Some(best) = pick_rock_tier(self.tiers, self.level) {
            if best.level > spot.level {
                bot.log("level up, switching tier");
                return false;
            }
        }
        if !rock_still_there(bot, at.0, at.1, spot) {
            bot.log(&format!("rock gone at ({},{})", at.0, at.1));
            state.banned.ban_for(at, bot.world.ticks, GONE_BAN_TICKS);
            return false;
        }
        true
    }

    fn work(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::kit::load::LoopState,
        _spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError> {
        work_spot(bot, at.0, at.1, &self.ore_ids, had)
    }
}

pub fn mine_load(
    bot: &mut Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    level: i64,
) -> Result<(bool, i64), ScriptError> {
    let ore_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut state = crate::kit::load::LoopState::fresh(bot);
    loop {
        if PICK_SPEC.best_held(bot).is_none() {
            // Recovery walks (shop, bank) are not mining: don't bill
            // them to the no-ore stall budget.
            let spent = bot.world.ticks;
            if !recover_pickaxe(bot, coins, bank)? {
                bot.log("pickaxe lost, banking load");
                return Ok((true, level));
            }
            state.idle_since += bot.world.ticks - spent;
        }
        let mut ops = MineOps { tiers, bank, coins, ore_ids: ore_ids.clone(), level };
        if crate::kit::load::run_load(bot, &crate::kit::load::LoadCfg::gather(), &mut state, &mut ops)? {
            return Ok((true, learn_level(bot, MINING_STAT, ops.level)));
        }
        if PICK_SPEC.best_held(bot).is_none() {
            continue;
        }
        return Ok((false, learn_level(bot, MINING_STAT, ops.level)));
    }
}
