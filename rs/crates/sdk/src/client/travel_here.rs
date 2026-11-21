//! Zero-distance trips answered without the plan service (CPU
//! 2026-11-14: 1 % of the live plans asked for the tile the bot stands
//! on, each one a socket round trip, an options read and a service
//! answer).
//!
//! The service answers such a request with the empty plan at cost 0
//! (`nav::fast_hop`: the planner pops the start first). The bot stands on
//! the tile, so the server holds it walkable: the same empty plan is
//! built here, and executing it is a no-op. The options are only read
//! by a replan, which an empty plan never triggers: the defaults with
//! the bot's danger squares, no `wants` round trip.

use lifescape_nav::plan::{NavOptions, Plan, Tile};

use super::travel::PlanTimings;
use crate::client::Bot;
use crate::error::{Error, Result};

/// Ticks a bot may keep moving before a plan is asked: a leg step of the
/// previous walk (run: two tiles a tick) ends well inside it; past it the
/// bot never stops and the plan is refused instead of drawn from a moving
/// start.
pub(crate) const SETTLE_MAX_TICKS: u64 = 30;

impl Bot {
    /// Pump until the bot stands still: its tile unchanged over one whole
    /// tick (the player-info packet carries the server-side steps, so an
    /// unchanged tile after a tick means the walk of the previous step
    /// ended). A plan asked before that starts behind the bot
    /// (`travel_drift_tests`). Bounded by `SETTLE_MAX_TICKS`: a bot still
    /// moving past it is `Error::Timeout`, never a plan from a moving start.
    pub(crate) fn wait_until_still(&mut self) -> Result<()> {
        // No game session (offline plan, bare bot): nothing can move.
        if !self.conn.is_open() {
            return Ok(());
        }
        let deadline = self.world.ticks + SETTLE_MAX_TICKS;
        loop {
            let before = (self.world.x, self.world.z, self.world.level);
            let tick = self.world.ticks;
            while self.world.ticks == tick {
                self.pump(1)?;
            }
            let after = (self.world.x, self.world.z, self.world.level);
            if before == after {
                return Ok(());
            }
            if self.world.ticks >= deadline {
                return Err(Error::Timeout(format!(
                    "bot never stood still for {SETTLE_MAX_TICKS} ticks before planning: {before:?} -> {after:?}"
                )));
            }
        }
    }

    /// The trip when `dest` is the bot's own tile, else `None`.
    pub(crate) fn plan_here(&self, dest: Tile) -> Option<(NavOptions, Plan, PlanTimings)> {
        let here = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        if here != dest {
            return None;
        }
        let options = NavOptions { f2p_only: true, danger: self.nav_danger.clone(), ..NavOptions::default() };
        let plan = Plan { segments: Vec::new(), cost: 0, adjusted_dest: None, dest_blocker: None };
        Some((options, plan, PlanTimings::default()))
    }
}

#[cfg(test)]
mod tests {
    use crate::client::Bot;
    use lifescape_nav::plan::Tile;

    /// The bot's own tile plans empty with no service at all (a remote
    /// plan without a service handle is `nav-down`).
    #[test]
    fn own_tile_plans_empty_without_the_service() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        (bot.world.x, bot.world.z, bot.world.level) = (3222, 3218, 0);
        assert!(bot.plan_client.is_none(), "no service handle");
        let (options, plan, _) = bot.plan_travel(Tile { x: 3222, z: 3218, level: 0 }).expect("own tile: no round trip");
        assert!(plan.segments.is_empty() && plan.cost == 0 && plan.adjusted_dest.is_none());
        assert!(options.f2p_only);
        let elsewhere = bot.plan_travel(Tile { x: 3223, z: 3218, level: 0 });
        assert!(matches!(elsewhere, Err(crate::error::Error::Travel(ref reason)) if reason.starts_with("nav-down")), "{elsewhere:?}");
        assert!(bot.plan_here(Tile { x: 3222, z: 3218, level: 1 }).is_none(), "another level is a trip");
    }
}

/// True when the bot left the plan's start by more than one tile (or
/// changed level) while the plan was computed: a leg still running
/// server-side (`travel_drift_tests`).
pub(crate) fn drifted(from: Tile, here: Tile) -> bool {
    from.level != here.level || (from.x - here.x).abs() > 1 || (from.z - here.z).abs() > 1
}

#[cfg(test)]
#[path = "travel_drift_tests.rs"]
mod drift_tests;
