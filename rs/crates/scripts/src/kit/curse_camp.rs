#![forbid(unsafe_code)]
//! The curse camp's pure rules (split from `activities/curse.rs`, the
//! activity cap): server refusals, per-run foe memory, the step choice
//! and the per-cast budget. No network: the activity reads the world
//! and acts.

use std::collections::HashMap;


/// Ticks a cast gets to show its XP (walk into range, 5-tick delay).
pub const CURSE_LAND_TICKS: u64 = 20;
/// Ticks a lowered foe stays out of the picks (one NPC regen period).
pub const LOWERED_BAN_TICKS: u64 = 100;
/// Ticks a foe taken by someone else, or unreachable, stays out.
pub const TAKEN_BAN_TICKS: u64 = 30;
/// Ticks a retaliating foe gets to die before the loop moves on.
pub const FINISH_TICKS: u64 = 150;
/// Ticks no cast is tried after "I'm already under attack!": the server
/// refuses every other target while a foe hit us within the last 8 ticks
/// (`player_combat.rs2` `player_in_combat_check`, `%lastcombat + 8`).
pub const UNDER_ATTACK_HOLD_TICKS: u64 = 8;
/// Picks one cast may try before it fails loud.
pub const PICKS_PER_CAST: u32 = 12;
/// Ticks one cast may wait for its XP (regen of the camp's lowered foes,
/// an aggressor finished, the cast's own landing) before it fails loud.
pub const CAST_WAIT_TICKS: u64 = LOWERED_BAN_TICKS + FINISH_TICKS + CURSE_LAND_TICKS;

/// Why a cast gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastSpent {
    /// `PICKS_PER_CAST` clicked casts paid no XP.
    Picks,
    /// `CAST_WAIT_TICKS` passed without XP (the camp never regenerated).
    Ticks,
}

/// Budget of one cast: only a clicked cast that paid no XP spends a pick
/// (finishing an aggressor, walking or waiting for the camp to
/// regenerate never does); the wait itself is bounded in ticks.
/// alice_knight 2026-11-17: 3 cows, each lowered by our own casts, the
/// finish and search steps ate the 12 picks in ~130 ticks.
#[derive(Debug, Clone)]
pub struct CastBudget {
    refused: u32,
    started: u64,
}

impl CastBudget {
    pub fn new(now: u64) -> CastBudget {
        CastBudget { refused: 0, started: now }
    }

    /// Account one step before it runs; `Err` when the cast is spent.
    pub fn step(&mut self, step: CurseStep, now: u64) -> Result<(), CastSpent> {
        if now.saturating_sub(self.started) >= CAST_WAIT_TICKS {
            return Err(CastSpent::Ticks);
        }
        if matches!(step, CurseStep::Cast(_)) && self.refused >= PICKS_PER_CAST {
            return Err(CastSpent::Picks);
        }
        Ok(())
    }

    /// A clicked cast that paid no XP.
    pub fn refused(&mut self) {
        self.refused += 1;
    }

    /// A market pause closed, or the caster walked back to the camp (the
    /// pause left it at the Varrock square): neither is a wait for the
    /// regeneration, the bound and the picks start again from `now`.
    pub fn restart(&mut self, now: u64) {
        self.started = now;
        self.refused = 0;
    }
}
/// Tiles from the bot within which a camp NPC is a candidate.
pub const FOE_RADIUS: i32 = 15;

/// Server answer to a curse click, by what the loop does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurseRefusal {
    /// The foe's stat is already lowered, or it is held or immune: out
    /// until it regenerates.
    Lowered,
    /// Another foe attacks us (single-way camp): deal with it first.
    UnderAttack,
    /// Another player fights it, or it is out of reach: skip it.
    Taken,
}

/// Classify one server line after a curse click (`None`: not a refusal).
pub fn curse_refusal(line: &str) -> Option<CurseRefusal> {
    let lowered = line.to_lowercase();
    if lowered.contains("has already been lowered") || lowered.contains("has already been weakened") || lowered.contains("already held by a magical force") || lowered.contains("currently immune") {
        Some(CurseRefusal::Lowered)
    } else if lowered.contains("already under attack") {
        Some(CurseRefusal::UnderAttack)
    } else if lowered.contains("someone else is fighting") || lowered.contains("can't reach that") || lowered.contains("not after you") {
        Some(CurseRefusal::Taken)
    } else {
        None
    }
}

/// Per-run memory of the camp's foes (never persisted): slots out of the
/// picks until a tick, and the slots our casts touched.
#[derive(Debug, Default, Clone)]
pub struct CurseFoes {
    banned: HashMap<u16, u64>,
    lowered: HashMap<u16, u64>,
    /// No cast before this tick (an unseen foe attacks us).
    hold_until: u64,
}

impl CurseFoes {
    /// Keep `slot` out of the picks until `until`.
    pub fn ban(&mut self, slot: u16, until: u64) {
        let known = self.banned.entry(slot).or_insert(until);
        *known = (*known).max(until);
    }

    /// The foe's stat is down (a refusal said so): finish it if it
    /// attacks, never cast on it until it regenerates.
    pub fn mark_lowered(&mut self, slot: u16, now: u64) {
        self.lowered.insert(slot, now + LOWERED_BAN_TICKS);
        self.ban(slot, now + LOWERED_BAN_TICKS);
    }

    /// "I'm already under attack!": a foe we do not see hits us; no
    /// other target is legal until the server's combat window lapses.
    pub fn under_attack(&mut self, now: u64) {
        self.hold_until = self.hold_until.max(now + UNDER_ATTACK_HOLD_TICKS);
    }

    /// True while the combat window of `under_attack` runs.
    pub fn is_held(&self, now: u64) -> bool {
        self.hold_until > now
    }

    pub fn is_lowered(&self, slot: u16, now: u64) -> bool {
        self.lowered.get(&slot).is_some_and(|until| *until > now)
    }

    pub fn is_banned(&self, slot: u16, now: u64) -> bool {
        self.banned.get(&slot).is_some_and(|until| *until > now)
    }
}

/// What the loop does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurseStep {
    /// A lowered foe attacks us: melee it down first.
    Finish(u16),
    /// Cast on this foe.
    Cast(u16),
    /// No candidate in sight: walk to the camp or wait.
    Search,
    /// An unseen foe attacks us: no cast until the combat window lapses.
    Wait,
}

/// Pure choice: an aggressor first (the only legal target while it hits
/// us), finished when lowered, skipped while a cast on it went
/// unanswered (banned: a stale or unreachable face, live 2026-11-19 slot
/// 405 at River Lum); no cast while an unseen foe holds us
/// (`under_attack`); else the nearest camp foe not banned.
pub fn choose_step(aggressor: Option<u16>, candidates: &[(u16, i32, i32)], here: (i32, i32), foes: &CurseFoes, now: u64) -> CurseStep {
    if let Some(slot) = aggressor {
        if foes.is_lowered(slot, now) {
            return CurseStep::Finish(slot);
        }
        if !foes.is_banned(slot, now) {
            return CurseStep::Cast(slot);
        }
    }
    if foes.is_held(now) {
        return CurseStep::Wait;
    }
    candidates
        .iter()
        .filter(|(slot, x, z)| !foes.is_banned(*slot, now) && (x - here.0).abs().max((z - here.1).abs()) <= FOE_RADIUS)
        .min_by_key(|(slot, x, z)| ((x - here.0).abs() + (z - here.1).abs(), *slot))
        .map_or(CurseStep::Search, |(slot, _, _)| CurseStep::Cast(*slot))
}
/// Pure: no camp foe is left to curse (`workable` 0) and other casters
/// hold some (`by_others`): the camp is contended, never a broken method
/// (typed `contended:<site>` when other players are there). Foes only
/// our own casts lowered wait for their regeneration instead.
pub fn all_taken(by_others: usize, workable: usize) -> bool {
    by_others > 0 && workable == 0
}
