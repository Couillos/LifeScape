//! Mine driver: the rock work loop (LoopOps) on the shared harvest
//! core, pickaxe recovery and banked restores. The pool (tiers, spawns,
//! liveness) lives in `kit::spots`; the activity keeps only the CLI and
//! the loads loop.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::deplete::{DepletionClock, RESPAWN_MARGIN_TICKS};
use crate::kit::spots::{pick_rock_spot, pick_rock_tier, rock_depleted_seen, rock_live_loc_id, rock_still_there, rock_tier_spawns, RockPick};
use crate::kit::tools::{recover_pickaxe, PICK_SPEC};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::harvest::WorkOutcome;
use crate::kit::survival::learn_level;
use crate::skill_walk::walk_checked;
use crate::ScriptError;

pub const MINING_STAT: u8 = 14;

/// Server start-of-work ack: the action loop runs on the server's side.
/// Mining-only vocabulary (point 1: skill-specific stays with the skill).
pub const MINING_ACKS: [&str; 1] = ["swing your pick"];

/// Click once, then wait for the swing loop (`mining.rs2`: `p_oploc(3)`
/// until an ore). Rock liveness is the loc id match: an ore depletes the
/// rock (`loc_change` to its empty stage) and the stand ends at once on
/// the streamed change, never a click on the empty rock; a gem ends the
/// action on a live rock (`return` without `p_oploc`), clicked again at
/// once. An acked swing on a still-matching rock is a dry streak: no
/// re-click (the woodcut rule, `harvest::reclick_after_silence`).
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

    /// The rock streamed another loc (its empty stage): mined out.
    fn depleted(&self, bot: &mut Bot) -> bool {
        bot.world.loc_at(self.x, self.z, bot.world.level).is_some_and(|(id, _, _)| id != self.loc)
    }

    /// Every ore or gem ends the server's action (`get_ore_*` returns).
    fn gain_ends_action(&self) -> bool {
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
    until: Option<crate::kit::cycle::WantGoal>,
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
    // A side of the rock, never a corner: the client routes, so the server
    // never walks a diagonal stand to the rock ("I can't reach that!").
    let (width, length) = crate::kit::harvest::spot_footprint(bot, rock_x, rock_z);
    let stands = rock_stands(bot.world.x, bot.world.z, rock_x, rock_z, width, length);
    let stands = crate::skill_walk::routed_stands(bot, stands);
    if !stands.is_empty() {
        let mut stood = false;
        for (stand_x, stand_z) in stands {
            match bot.walk_to(stand_x, stand_z, bot.world.level, 100) {
                Ok(()) if crate::kit::harvest::operable_stand(bot.world.x, bot.world.z, rock_x, rock_z, width, length) => {
                    stood = true;
                    break;
                }
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error,
                        lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                    ) =>
                {
                    return Err(error.into())
                }
                Err(_) => {}
            }
        }
        if !stood {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let rock_loc = rock_live_loc_id(bot, rock_x, rock_z)?;
    let mut source = RockSource { loc: rock_loc, x: rock_x, z: rock_z };
    crate::kit::harvest::watch_stand(bot, &mut source, rock_x, rock_z, ore_ids, had_ore, until)
}

/// Stands to try for a rock, nearest first: the side middles of its
/// footprint (four orthogonal tiles for a 1x1 rock). Empty when the bot
/// already stands on a side. The server reaches a centrepiece loc
/// orthogonally only and, client routefinder on, never paths a player
/// to a loc: a corner stand is "I can't reach that!" on every click.
pub(crate) fn rock_stands(bot_x: i32, bot_z: i32, rock_x: i32, rock_z: i32, width: i64, length: i64) -> Vec<(i32, i32)> {
    if crate::kit::harvest::operable_stand(bot_x, bot_z, rock_x, rock_z, width, length) {
        return Vec::new();
    }
    let mut ring = crate::kit::harvest::operable_ring(rock_x, rock_z, width, length);
    ring.sort_by_key(|tile| ((tile.0 - bot_x).abs() + (tile.1 - bot_z).abs(), *tile));
    ring
}

/// Mining policy on the shared driver: full pack ends the load, a lost
/// pickaxe exits for caller-side recovery (reattach, shop, bank), tier
/// upgrades and dead rocks drop the spot, a mined-out site waits for its
/// soonest respawn or moves (`kit::deplete`).
struct MineOps<'a> {
    tiers: &'a [knowledge::rocks::RockTier],
    bank: &'a knowledge::banks::Bank,
    coins: u16,
    ore_ids: Vec<u16>,
    level: i64,
    /// Depletion stamps of the run (`kit::deplete`).
    clock: &'a mut DepletionClock,
    /// The last pick's mined-out rocks (`RockPick::Wait`): the tier and
    /// its depleted rocks, read by `on_no_spot`.
    depleted: Option<(knowledge::rocks::RockTier, Vec<(i32, i32)>)>,
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
        self.depleted = None;
        let found = pick_rock_spot(bot, self.tiers, self.bank, self.level, &state.banned, bot.world.ticks);
        match found {
            Some(RockPick::Go(tier, spot_x, spot_z)) => {
                bot.log(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
                bot.intend(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
                Some((tier, spot_x, spot_z))
            }
            // Mined out: `on_no_spot` weighs the respawn against the
            // brain's other site.
            Some(RockPick::Wait(tier, rocks)) => {
                self.depleted = Some((tier, rocks));
                None
            }
            None => {
                bot.log("no rock spot, waiting for respawns");
                bot.intend("waiting for a safe rock spot");
                None
            }
        }
    }

    /// Every rock of the tier mined out: a crowd moves on (user
    /// 2026-11-19), else the soonest respawn against the walk to the
    /// brain's other site plus its own wait (user 2026-11-20).
    fn on_no_spot(&mut self, bot: &mut Bot, state: &mut crate::kit::load::LoopState) -> Result<crate::kit::load::NoSpot, ScriptError> {
        use crate::kit::deplete::DepletedChoice;
        use crate::kit::load::NoSpot;
        let Some((tier, rocks)) = self.depleted.take() else { return Ok(NoSpot::Idle) };
        // Too many players for the rocks (user 2026-11-19): the brain
        // moves the bot to another site, never a long respawn wait in a
        // crowd.
        if crowded(crate::kit::site::players_near(bot), rocks.len()) {
            if let Some(contended) = crate::kit::site::contended_error(bot) {
                bot.log(&format!("{} {} rocks mined out among {} players: changing spot", rocks.len(), tier.product, crate::kit::site::players_near(bot)));
                return Err(contended);
            }
        }
        let now = bot.world.ticks;
        let rate = rock_rate(&tier);
        for rock in &rocks {
            self.clock.observe(*rock, true, now);
        }
        let Some(wait_here) = self.clock.soonest(&rocks, rate, now) else { return Ok(NoSpot::Idle) };
        let (clock, bank) = (&mut *self.clock, self.bank);
        let choice = crate::kit::deplete::decide(bot, &tier.product, rocks.len(), wait_here, |bot, site| site_rocks_wait(bot, &tier, bank, site, clock));
        if let (DepletedChoice::Move(_), Some(site)) = (choice, bot.assigned_site) {
            return Err(crate::kit::deplete::depleted_error(site, wait_here));
        }
        let spent = bot.world.ticks;
        let watch = wait_rock_respawn(bot, &tier, &rocks, wait_here, self.clock)?;
        state.idle_since += bot.world.ticks - spent;
        Ok(if watch == RespawnWatch::Hurt { NoSpot::EndLoad } else { NoSpot::Repick })
    }

    /// A rock the ore just mined out (stream shows its empty stage) is
    /// no ban: the pool skips it while empty and takes it back the tick
    /// it respawns. Any other outcome keeps the shared map (a « no ore »
    /// the stream does not confirm still bans, never a click loop).
    #[allow(clippy::too_many_arguments)]
    fn on_outcome(
        &mut self,
        bot: &mut Bot,
        cfg: &crate::kit::load::LoadCfg,
        state: &mut crate::kit::load::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
        outcome: WorkOutcome,
        gained: u32,
    ) -> Result<crate::kit::load::LoadExit, ScriptError> {
        if outcome == WorkOutcome::Gone && rock_depleted_seen(bot, at.0, at.1, spot) {
            // Seen mined out: its respawn clock starts now. Every rock
            // near mined out, the next pick hands the site to
            // `on_no_spot` (never a click on an empty rock, never a
            // 250-tick ban on a 20-tick rock).
            self.clock.observe(at, true, bot.world.ticks);
            state.spot = None;
            self.level = learn_level(bot, MINING_STAT, self.level);
            return Ok(crate::kit::load::stall_or_continue(cfg, state, gained, bot.world.ticks));
        }
        Ok(crate::kit::load::apply_outcome(cfg, state, at, outcome, gained, bot.world.ticks, self.on_gone(), self.on_refusal(), self.on_no_tool()))
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::kit::load::LoopState,
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
            // Mined out under us: the pool skips it until it respawns.
            bot.log(&format!("rock gone at ({},{})", at.0, at.1));
            return false;
        }
        true
    }

    fn work(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::kit::load::LoopState,
        _spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError> {
        work_spot(bot, at.0, at.1, &self.ore_ids, had, state.until)
    }
}

/// How a respawn watch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RespawnWatch {
    Respawned,
    TimedOut,
    Hurt,
}

/// Watch the depleted `rocks` until one respawns (stream shows its tier
/// rock again), at most `wait` plus a margin; a hit breaks the wait (the
/// caller's hurt rules own it), random events are polled on the way, a
/// rock seen standing again leaves the depletion clock.
fn wait_rock_respawn(bot: &mut Bot, tier: &knowledge::rocks::RockTier, rocks: &[(i32, i32)], wait: u64, clock: &mut DepletionClock) -> Result<RespawnWatch, ScriptError> {
    bot.log(&format!("{} {} rocks mined out, waiting a respawn (<= {wait}t)", rocks.len(), tier.product));
    bot.intend(&format!("waiting for a {} rock to respawn", tier.product));
    let hitpoints_before = crate::kit::survival::hitpoints(bot);
    let mut polled = bot.world.ticks;
    let until = polled + wait + RESPAWN_MARGIN_TICKS;
    while bot.world.ticks < until {
        bot.pump(1)?;
        if bot.world.ticks >= polled + 20 {
            polled = bot.world.ticks;
            crate::kit::survival::check_events(bot)?;
        }
        if crate::kit::survival::hitpoints(bot) < hitpoints_before {
            return Ok(RespawnWatch::Hurt);
        }
        let standing: Vec<(i32, i32)> = rocks.iter().copied().filter(|(x, z)| !rock_depleted_seen(bot, *x, *z, tier)).collect();
        if !standing.is_empty() {
            for rock in standing {
                clock.observe(rock, false, bot.world.ticks);
            }
            return Ok(RespawnWatch::Respawned);
        }
    }
    Ok(RespawnWatch::TimedOut)
}

/// Server respawn of the tier's rocks (`rock_respawnrate`), 0 for a rock
/// that never depletes.
fn rock_rate(tier: &knowledge::rocks::RockTier) -> u64 {
    knowledge::action_odds::rock_respawn(&tier.product).unwrap_or(0).max(0) as u64
}

/// The other site's own wait: its soonest respawn when the bot sees every
/// rock of the tier there mined out, else 0 (nothing known against it).
fn site_rocks_wait(bot: &mut Bot, tier: &knowledge::rocks::RockTier, bank: &knowledge::banks::Bank, site: (i32, i32, i32), clock: &mut DepletionClock) -> u64 {
    if site.2 != bot.world.level {
        return 0;
    }
    let rocks: Vec<(i32, i32)> = rock_tier_spawns(tier, bank, Some(site))
        .into_iter()
        .filter(|(x, z)| crate::kit::site::within_site((*x, *z, site.2), site))
        .collect();
    if rocks.is_empty() || !rocks.iter().all(|(x, z)| rock_depleted_seen(bot, *x, *z, tier)) {
        return 0;
    }
    let now = bot.world.ticks;
    for rock in &rocks {
        clock.observe(*rock, true, now);
    }
    clock.soonest(&rocks, rock_rate(tier), now).unwrap_or(0)
}

/// True when the other players around (`players`) are at least as many
/// as the site's rocks of the tier (`rocks`): a crowd, the bot changes
/// spot instead of waiting a respawn (user 2026-11-19).
fn crowded(players: usize, rocks: usize) -> bool {
    players >= rocks.max(1)
}

pub fn mine_load(
    bot: &mut Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    level: i64,
    until: Option<crate::kit::cycle::WantGoal>,
    clock: &mut DepletionClock,
) -> Result<(bool, i64), ScriptError> {
    let ore_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut state = crate::kit::load::LoopState::fresh(bot);
    // The run's `--want` goal ends the load before the pack fills.
    let cfg = crate::kit::load::LoadCfg::gather().until(until);
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
        let mut ops = MineOps { tiers, bank, coins, ore_ids: ore_ids.clone(), level, clock: &mut *clock, depleted: None };
        if crate::kit::load::run_load(bot, &cfg, &mut state, &mut ops)? {
            return Ok((true, learn_level(bot, MINING_STAT, ops.level)));
        }
        if PICK_SPEC.best_held(bot).is_none() {
            continue;
        }
        return Ok((false, learn_level(bot, MINING_STAT, ops.level)));
    }
}

#[cfg(test)]
mod tests {
    /// User 2026-11-19 (« si y'a trop de joueurs il faut changer de
    /// spot »): 17 miners on the 4 open adamantite rocks waited out the
    /// long respawns. Rocks mined out with as many other players as rocks
    /// around: the site is crowded, the bot moves on; fewer players than
    /// rocks, the respawn is worth the wait.
    #[test]
    fn as_many_players_as_rocks_is_a_crowd() {
        assert!(super::crowded(2, 2));
        assert!(super::crowded(5, 2));
        assert!(!super::crowded(1, 2));
        assert!(!super::crowded(0, 1));
        assert!(super::crowded(1, 0), "no rock left at all");
    }

    use super::rock_stands;
    use crate::kit::harvest::operable_stand;

    /// Live 2026-11-13 (frank_lane Varrock SW iron, fiona_park Rimmington
    /// gold, bbkjz2vk Barbarian coal): the walk settled diagonal to the
    /// rock, `step_adjacent` kept that tile (any of the 8 neighbours) and
    /// every click answered "I can't reach that!" (client routefinder:
    /// the server never paths to a loc, and a centrepiece loc is reached
    /// orthogonally only). The rock stands are the four sides, nearest
    /// first; none when the bot already stands on a side.
    #[test]
    fn diagonal_stand_rotates_to_a_side() {
        let stands = rock_stands(3176, 3367, 3175, 3368, 1, 1);
        assert_eq!(stands.len(), 4, "four sides, got {stands:?}");
        assert!(stands.iter().all(|(x, z)| operable_stand(*x, *z, 3175, 3368, 1, 1)), "sides only, got {stands:?}");
        let first_two: Vec<(i32, i32)> = stands[..2].to_vec();
        assert!(first_two.contains(&(3175, 3367)) && first_two.contains(&(3176, 3368)), "nearest sides first, got {stands:?}");
        assert!(rock_stands(3176, 3368, 3175, 3368, 1, 1).is_empty(), "east side already operable");
        assert!(rock_stands(3175, 3367, 3175, 3368, 1, 1).is_empty(), "south side already operable");
    }
}
