//! Shared gather load driver: one state, one threshold set, one outcome
//! map. Mining, fishing and woodcut all run through `run_load`; per-skill
//! quirks (tier switches, bait restocks, refusal strikes, respawn waits)
//! live in their `LoopOps` impls, never in the driver.
//! Picks take `&mut Bot` (the woodcut walk-cost refinement plans); walks,
//! pumps and sends live in `pick`/`work`/hooks only.
//! State sits outside `Bot`; tiers are `Clone`, never `Copy`.

#![forbid(unsafe_code)]

use lifescape_sdk::waits::{wait_hitpoints, wait_ticks};
use lifescape_sdk::Bot;

use crate::skill_common::{check_events, hitpoints, WorkOutcome};
use crate::skill_gather::{death_since, note_gain, stall_expired, BanSet};
use crate::ScriptError;

/// Every tick and ban bound the loops hard-coded, in one place.
pub struct LoadCfg {
    pub stall_ticks: u64,
    pub gone_ban: u64,
    pub structural_ban: u64,
    pub unreach_ban: u64,
    pub refusal_ban: u64,
    pub flee_cap: u32,
    pub flee_ban: u64,
    pub flee_wait: u64,
    pub death_wait: u64,
    pub repick_tries: u32,
    pub repick_wait: u64,
    pub round_idle: u64,
}

impl LoadCfg {
    /// The values the three loops already run.
    pub fn mining() -> LoadCfg {
        LoadCfg {
            stall_ticks: 1000, gone_ban: 250, structural_ban: 7200, unreach_ban: 1800, refusal_ban: 500,
            flee_cap: 2, flee_ban: 500, flee_wait: 200, death_wait: 40,
            repick_tries: 16, repick_wait: 15, round_idle: 12,
        }
    }
}

/// Minimal common loop state (extensions stay caller-side).
pub struct LoopState {
    pub banned: BanSet,
    pub idle_since: u64,
    pub last_hp: u8,
    pub flees: u32,
    pub seen_death: usize,
    pub spot: Option<(i32, i32)>,
}

impl LoopState {
    pub fn fresh(bot: &Bot) -> LoopState {
        LoopState {
            banned: BanSet::new(),
            idle_since: bot.world.ticks,
            last_hp: hitpoints(bot),
            flees: 0,
            seen_death: bot.world.messages.len(),
            spot: None,
        }
    }

    /// Resume with bans that outlive one load (a killer tree stays banned
    /// while its neighbours get cut instead of repicking the same death).
    pub fn resume(bot: &Bot, banned: BanSet) -> LoopState {
        let mut state = LoopState::fresh(bot);
        state.banned = banned;
        state
    }
}

/// Pure outcome verdict: continue, load done, or abort.
pub enum LoadExit {
    Continue,
    Full,
    Abort,
}

/// Gone ban: depletion, quiet spot, threat. Short re-probe delay.
pub fn ban_gone(bans: &mut BanSet, at: (i32, i32), now: u64, cfg: &LoadCfg) {
    bans.ban_for(at, now, cfg.gone_ban);
}

/// Refusal ban: walk fell short, level gate. Never structural.
pub fn ban_refusal(bans: &mut BanSet, at: (i32, i32), now: u64, cfg: &LoadCfg) {
    bans.ban_for(at, now, cfg.refusal_ban);
}

/// No-op ban: a pack problem (lost tool), never a spot problem.
pub fn ban_none(_bans: &mut BanSet, _at: (i32, i32), _now: u64, _cfg: &LoadCfg) {}

/// Gain note plus stall bound: the shared tail of every outcome verdict.
pub fn stall_or_continue(cfg: &LoadCfg, state: &mut LoopState, gained: u32, now: u64) -> LoadExit {
    if gained > 0 {
        note_gain(&mut state.idle_since, now);
    }
    if stall_expired(now, state.idle_since, cfg.stall_ticks) {
        return LoadExit::Abort;
    }
    LoadExit::Continue
}

/// Pure outcome map (no walk/pump/send; `on_no_bait` runs before this, in
/// `LoopOps::on_outcome`, so it never reaches the match).
#[allow(clippy::too_many_arguments)]
pub fn apply_outcome(
    cfg: &LoadCfg,
    state: &mut LoopState,
    at: (i32, i32),
    outcome: WorkOutcome,
    gained: u32,
    now: u64,
    on_gone: fn(&mut BanSet, (i32, i32), u64, &LoadCfg),
    on_refusal: fn(&mut BanSet, (i32, i32), u64, &LoadCfg),
    on_no_tool: fn(&mut BanSet, (i32, i32), u64, &LoadCfg),
) -> LoadExit {
    match outcome {
        WorkOutcome::Gone | WorkOutcome::Threat | WorkOutcome::Moved => {
            on_gone(&mut state.banned, at, now, cfg);
            state.spot = None;
        }
        WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot | WorkOutcome::LevelLocked => {
            on_refusal(&mut state.banned, at, now, cfg);
            state.spot = None;
        }
        WorkOutcome::NoTool => {
            on_no_tool(&mut state.banned, at, now, cfg);
            state.spot = None;
        }
        WorkOutcome::NoBait => {
            state.spot = None;
        }
        WorkOutcome::Blocked => return LoadExit::Abort,
        WorkOutcome::Full => return LoadExit::Full,
    }
    stall_or_continue(cfg, state, gained, now)
}

/// Pure hurt accounting: ban the fled spot, count, abort past the cap.
pub fn register_hurt(cfg: &LoadCfg, state: &mut LoopState, now: u64) -> bool {
    if let Some(pos) = state.spot {
        state.banned.ban_for(pos, now, cfg.flee_ban);
    }
    state.flees += 1;
    state.flees > cfg.flee_cap
}

/// Per-skill loop policy: reads take `&Bot`, everything that walks,
/// pumps, bans or learns takes `&mut Bot`/`&mut self`. `Spot` is `Clone`,
/// never `Copy`.
pub trait LoopOps {
    type Spot: Clone;
    fn is_full(&self, bot: &Bot) -> bool;
    fn has_tool(&self, bot: &Bot) -> bool;
    fn gain_of(&self, bot: &Bot, spot: &Self::Spot) -> u32;
    fn pick(&mut self, bot: &mut Bot, state: &mut LoopState) -> Option<(Self::Spot, i32, i32)>;
    fn work(
        &mut self,
        bot: &mut Bot,
        state: &mut LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError>;
    fn on_gone(&self) -> fn(&mut BanSet, (i32, i32), u64, &LoadCfg) {
        ban_gone
    }
    fn on_refusal(&self) -> fn(&mut BanSet, (i32, i32), u64, &LoadCfg) {
        ban_refusal
    }
    /// Pack-problem ban: spot stays by default (a lost tool condemns
    /// nothing); fishing bans, exactly like its old loop.
    fn on_no_tool(&self) -> fn(&mut BanSet, (i32, i32), u64, &LoadCfg) {
        ban_none
    }
    /// Keep the current spot: tier upgrade switches and liveness re-probes
    /// drop it (banning when the old loop banned); default never drops.
    fn keep_spot(&mut self, _bot: &mut Bot, _state: &mut LoopState, _spot: &Self::Spot, _at: (i32, i32)) -> bool {
        true
    }
    /// Bait ran out mid-click: restock and resume the same spot when true,
    /// drop it when false. Only fishing restocks; the default drops.
    fn on_no_bait(
        &mut self,
        _bot: &mut Bot,
        _state: &mut LoopState,
        _spot: &Self::Spot,
        _at: (i32, i32),
    ) -> Result<bool, ScriptError> {
        Ok(false)
    }
    /// Hurt mid-load: ban the fled spot, count, wait the flee window, true
    /// past the cap. Only the wait flavor varies (fishing waits flat).
    fn on_hurt(&mut self, bot: &mut Bot, cfg: &LoadCfg, state: &mut LoopState) -> Result<bool, ScriptError> {
        bot.log("hurt, fleeing");
        bot.intend("fleeing a hunter");
        let abort = register_hurt(cfg, state, bot.world.ticks);
        wait_hitpoints(bot, hitpoints(bot).saturating_add(1), cfg.flee_wait)?;
        Ok(abort)
    }
    /// Post-work verdict: bait resumes through `on_no_bait`, everything
    /// else maps through `apply_outcome`. Woodcut overrides wholesale
    /// (respawn waits, refusal strikes, pocket escapes).
    #[allow(clippy::too_many_arguments)]
    fn on_outcome(
        &mut self,
        bot: &mut Bot,
        cfg: &LoadCfg,
        state: &mut LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
        outcome: WorkOutcome,
        gained: u32,
    ) -> Result<LoadExit, ScriptError> {
        if outcome == WorkOutcome::NoBait {
            let resumed = self.on_no_bait(bot, state, spot, at)?;
            if !resumed {
                state.spot = None;
            }
            return Ok(stall_or_continue(cfg, state, gained, bot.world.ticks));
        }
        Ok(apply_outcome(cfg, state, at, outcome, gained, bot.world.ticks, self.on_gone(), self.on_refusal(), self.on_no_tool()))
    }
}

/// Respawn wait after death: bounded, never billed to the stall budget.
fn respawn_wait(bot: &mut Bot, cfg: &LoadCfg, state: &mut LoopState) -> Result<(), ScriptError> {
    bot.log("died, respawning");
    bot.intend("died, respawning");
    let target = bot.world.ticks + cfg.death_wait;
    while bot.world.ticks < target && hitpoints(bot) == 0 {
        bot.pump(5)?;
    }
    state.last_hp = hitpoints(bot);
    state.seen_death = bot.world.messages.len();
    Ok(())
}

/// Shared load driver: full/tool gates, death, flee, tier/liveness drops,
/// repick, outcome bans. Recovery stays caller-side (`has_tool` false
/// exits `Ok(false)`; the caller re-gears and re-enters with the same
/// state, so bans survive).
pub fn run_load<Ops: LoopOps>(
    bot: &mut Bot,
    cfg: &LoadCfg,
    state: &mut LoopState,
    ops: &mut Ops,
) -> Result<bool, ScriptError> {
    let mut current: Option<(Ops::Spot, i32, i32)> = None;
    let mut had = 0u32;
    loop {
        check_events(bot)?;
        if ops.is_full(bot) {
            return Ok(true);
        }
        if !ops.has_tool(bot) {
            return Ok(false);
        }
        let (died, fresh) = death_since(bot, state.seen_death);
        state.seen_death = fresh;
        if died {
            let spent = bot.world.ticks;
            respawn_wait(bot, cfg, state)?;
            state.idle_since += bot.world.ticks - spent;
            state.spot = None;
            current = None;
            continue;
        }
        if hitpoints(bot) < state.last_hp {
            if ops.on_hurt(bot, cfg, state)? {
                return Ok(false);
            }
            state.spot = None;
            current = None;
            state.last_hp = hitpoints(bot);
            continue;
        }
        state.last_hp = hitpoints(bot);
        // Banned spots skip the keep check: the repick below covers them
        // (it re-learns the level and re-selects), so a liveness probe
        // never clobbers a longer ban with a short one.
        let drop_spot = match current.as_ref() {
            Some((spot, spot_x, spot_z)) if !state.banned.is_banned((*spot_x, *spot_z), bot.world.ticks) => {
                !ops.keep_spot(bot, state, spot, (*spot_x, *spot_z))
            }
            _ => false,
        };
        if drop_spot {
            state.spot = None;
            current = None;
        }
        let now = bot.world.ticks;
        state.banned.retain(now);
        let stale = state.spot.is_none_or(|spot| state.banned.is_banned(spot, now));
        if stale {
            let mut picked = None;
            for _ in 0..cfg.repick_tries {
                if let Some(found) = ops.pick(bot, state) {
                    picked = Some(found);
                    break;
                }
                wait_ticks(bot, cfg.repick_wait)?;
            }
            let Some((spot, spot_x, spot_z)) = picked else {
                return Ok(false);
            };
            had = ops.gain_of(bot, &spot);
            state.spot = Some((spot_x, spot_z));
            current = Some((spot, spot_x, spot_z));
        }
        let Some((spot, spot_x, spot_z)) = current.clone() else {
            state.spot = None;
            continue;
        };
        let (outcome, gained) = ops.work(bot, state, &spot, (spot_x, spot_z), had)?;
        let (died_mid, fresh_mid) = death_since(bot, state.seen_death);
        state.seen_death = fresh_mid;
        if died_mid {
            let spent = bot.world.ticks;
            respawn_wait(bot, cfg, state)?;
            state.idle_since += bot.world.ticks - spent;
            state.spot = None;
            current = None;
            continue;
        }
        had += gained;
        match ops.on_outcome(bot, cfg, state, &spot, (spot_x, spot_z), outcome, gained)? {
            LoadExit::Continue => {
                if state.spot.is_none() {
                    current = None;
                }
            }
            LoadExit::Full => return Ok(true),
            LoadExit::Abort => return Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_outcome, ban_gone, ban_none, ban_refusal, register_hurt, stall_or_continue, LoadCfg, LoadExit, LoopState};
    use crate::skill_common::WorkOutcome;
    use crate::skill_gather::BanSet;
    use crate::skill_loop::LoopOps;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Pure driver map: stall, flee and every ban class without network.
    #[test]
    fn outcomes_ban_stall_and_flee_pure() {
        let cfg = LoadCfg::mining();
        assert_eq!((cfg.stall_ticks, cfg.gone_ban, cfg.structural_ban, cfg.unreach_ban), (1000, 250, 7200, 1800));
        assert_eq!((cfg.flee_cap, cfg.flee_ban, cfg.refusal_ban), (2, 500, 500));
        assert_eq!((cfg.repick_tries, cfg.repick_wait, cfg.round_idle, cfg.death_wait, cfg.flee_wait), (16, 15, 12, 40, 200));
        let mut state = LoopState { banned: BanSet::new(), idle_since: 100, last_hp: 10, flees: 0, seen_death: 0, spot: Some((5, 5)) };
        assert!(matches!(apply_outcome(&cfg, &mut state, (5, 5), WorkOutcome::Gone, 0, 100, ban_gone, ban_refusal, ban_none), LoadExit::Continue));
        assert!(state.banned.is_banned((5, 5), 100 + cfg.gone_ban - 1) && state.spot.is_none());
        state.spot = Some((6, 6));
        assert!(matches!(apply_outcome(&cfg, &mut state, (6, 6), WorkOutcome::Threat, 0, 200, ban_gone, ban_refusal, ban_none), LoadExit::Continue)
            && state.banned.is_banned((6, 6), 200 + cfg.gone_ban - 1));
        state.spot = Some((7, 7));
        assert!(matches!(
            apply_outcome(&cfg, &mut state, (7, 7), WorkOutcome::UnreachableSpot, 0, 300, ban_gone, ban_refusal, ban_none),
            LoadExit::Continue
        ) && state.banned.is_banned((7, 7), 300 + cfg.refusal_ban - 1));
        assert!(!state.banned.is_banned((7, 7), 300 + cfg.structural_ban - 1), "one refusal never condemns");
        state.spot = Some((10, 10));
        assert!(matches!(
            apply_outcome(&cfg, &mut state, (10, 10), WorkOutcome::NoTool, 0, 310, ban_gone, ban_refusal, ban_none),
            LoadExit::Continue
        ) && state.spot.is_none());
        assert!(!state.banned.is_banned((10, 10), 311), "a lost tool condemns nothing by default");
        state.spot = Some((11, 11));
        assert!(matches!(
            apply_outcome(&cfg, &mut state, (11, 11), WorkOutcome::NoTool, 0, 320, ban_gone, ban_refusal, ban_refusal),
            LoadExit::Continue
        ) && state.banned.is_banned((11, 11), 320 + cfg.refusal_ban - 1));
        state.spot = Some((8, 8));
        assert!(matches!(apply_outcome(&cfg, &mut state, (8, 8), WorkOutcome::Moved, 2, 400, ban_gone, ban_refusal, ban_none), LoadExit::Continue)
            && state.idle_since == 400, "a gain resets the stall budget");
        assert!(matches!(
            apply_outcome(&cfg, &mut state, (8, 8), WorkOutcome::Gone, 0, 400 + cfg.stall_ticks + 1, ban_gone, ban_refusal, ban_none),
            LoadExit::Abort
        ), "quiet past the bound aborts");
        assert!(matches!(apply_outcome(&cfg, &mut state, (8, 8), WorkOutcome::Blocked, 0, 500, ban_gone, ban_refusal, ban_none), LoadExit::Abort));
        assert!(matches!(apply_outcome(&cfg, &mut state, (8, 8), WorkOutcome::Full, 0, 500, ban_gone, ban_refusal, ban_none), LoadExit::Full));
        let mut flee = LoopState { banned: BanSet::new(), idle_since: 0, last_hp: 10, flees: 0, seen_death: 0, spot: Some((9, 9)) };
        assert!(!register_hurt(&cfg, &mut flee, 600) && !register_hurt(&cfg, &mut flee, 601), "two flees ride");
        assert!(flee.banned.is_banned((9, 9), 601 + cfg.flee_ban - 1));
        assert!(register_hurt(&cfg, &mut flee, 602), "past the cap the load aborts");
    }

    /// Stall tail: a gain resets to now, expiry is strictly over the bound.
    #[test]
    fn stall_tail_resets_on_gain() {
        let cfg = LoadCfg::mining();
        let mut state =
            LoopState { banned: BanSet::new(), idle_since: 100, last_hp: 10, flees: 0, seen_death: 0, spot: Some((1, 1)) };
        assert!(matches!(stall_or_continue(&cfg, &mut state, 3, 150), LoadExit::Continue));
        assert_eq!(state.idle_since, 150, "a gain resets the budget");
        assert!(matches!(stall_or_continue(&cfg, &mut state, 0, 150 + 1000), LoadExit::Continue), "exactly on the bound still counts");
        assert!(matches!(stall_or_continue(&cfg, &mut state, 0, 150 + 1001), LoadExit::Abort), "one past the bound aborts");
    }

    /// Offline dummy: exercises the default hooks without any network.
    struct DummyOps;
    impl LoopOps for DummyOps {
        type Spot = ();
        fn is_full(&self, _bot: &Bot) -> bool {
            false
        }
        fn has_tool(&self, _bot: &Bot) -> bool {
            true
        }
        fn gain_of(&self, _bot: &Bot, _spot: &Self::Spot) -> u32 {
            0
        }
        fn pick(&mut self, _bot: &mut Bot, _state: &mut LoopState) -> Option<(Self::Spot, i32, i32)> {
            None
        }
        fn work(
            &mut self,
            _bot: &mut Bot,
            _state: &mut LoopState,
            _spot: &Self::Spot,
            _at: (i32, i32),
            _had: u32,
        ) -> Result<(WorkOutcome, u32), crate::ScriptError> {
            Ok((WorkOutcome::Gone, 0))
        }
    }

    /// Default hooks: keep-all spots, drop-on-NoBait, default outcome map.
    #[test]
    fn default_hooks_keep_and_drop() {
        let cfg = LoadCfg::mining();
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let mut ops = DummyOps;
        let mut state =
            LoopState { banned: BanSet::new(), idle_since: 0, last_hp: 10, flees: 0, seen_death: 0, spot: Some((4, 4)) };
        assert!(ops.keep_spot(&mut bot, &mut state, &(), (4, 4)), "default keeps every spot");
        assert!(!ops.on_no_bait(&mut bot, &mut state, &(), (4, 4)).expect("default never fails"), "default drops on NoBait");
        state.spot = Some((4, 4));
        let exit = ops.on_outcome(&mut bot, &cfg, &mut state, &(), (4, 4), WorkOutcome::Gone, 0).expect("pure map");
        assert!(matches!(exit, LoadExit::Continue) && state.spot.is_none(), "Gone bans and drops");
        assert!(state.banned.is_banned((4, 4), cfg.gone_ban - 1));
        state.spot = Some((5, 5));
        let exit = ops.on_outcome(&mut bot, &cfg, &mut state, &(), (5, 5), WorkOutcome::NoBait, 0).expect("pure map");
        assert!(matches!(exit, LoadExit::Continue) && state.spot.is_none(), "unresumed bait drops without a ban");
        assert!(!state.banned.is_banned((5, 5), 1), "unresumed bait bans nothing");
        let exit = ops.on_outcome(&mut bot, &cfg, &mut state, &(), (4, 4), WorkOutcome::Full, 0).expect("pure map");
        assert!(matches!(exit, LoadExit::Full));
        let exit = ops.on_outcome(&mut bot, &cfg, &mut state, &(), (4, 4), WorkOutcome::Blocked, 0).expect("pure map");
        assert!(matches!(exit, LoadExit::Abort));
    }

    /// Resume carries bans across loads (a killer tree stays banned while
    /// its neighbours get cut); fresh starts clean.
    #[test]
    fn resume_carries_bans_across_loads() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let mut bans = BanSet::new();
        bans.ban_for((9, 9), 0, 500);
        let resumed = LoopState::resume(&bot, bans);
        assert!(resumed.banned.is_banned((9, 9), 499), "threat ban outlives the load");
        assert!(resumed.spot.is_none(), "no stale spot across loads");
        let fresh = LoopState::fresh(&bot);
        assert!(!fresh.banned.is_banned((9, 9), 1), "a fresh load starts clean");
    }

    /// Golden corridors (one per skill): bank stand to resource tile plans
    /// offline, the class-A proof for the three loops.
    #[test]
    fn golden_corridors_cover_three_skills() {
        use lifescape_nav::plan::{NavOptions, Tile};
        let tile = |(x, z, level): (i32, i32, i32)| Tile { x, z, level };
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        for (skill, from, dest) in
            [("mine", (3253, 3420, 0), (3282, 3367, 0)), ("fish", (3092, 3243, 0), (3085, 3230, 0)), ("wood", (3092, 3243, 0), (3057, 3251, 0))]
        {
            let plan = lifescape_nav::plan::plan(&world, &tables, tile(from), tile(dest), &NavOptions::default());
            assert!(plan.is_ok(), "{skill} corridor {from:?}->{dest:?} must plan, got: {plan:?}");
        }
    }
}
