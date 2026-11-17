//! What the planner reads besides the game data when its caller holds
//! more than the game prior (plan rendements R7, decisions 2026-11-17
//! (b) and (c)):
//! - `rate`: the yield of a method at a level (`work_rate::RateReader`):
//!   the brain hands its MEASURED reader (the estimator's blend of the
//!   runs and the prior, `orchestrator::brain_plan_read`), offline callers
//!   and tests the game prior (`PlanRead::prior`). Sizing, the skeleton's
//!   obtains, a tranche's duration and the block's obtain counts read it;
//! - `guide_trains`: true when a level tranche (skill, from, to) will
//!   pick its method from the training guide at resolve (a line within
//!   the bot's reach in the published guide): the skeleton then files
//!   nothing of the static tranche method ahead (its tools, what it
//!   burns), the guide's pick blocks on what IT lacks
//!   (`expand::lock_training_pick`). Never true offline.
//!
//! The functions taking it are the `*_with` variants of the planner's
//! expansion and replanning (`expand::expand_goal_with`,
//! `lifecycle::record_block_with`, `goals::append_goal_with`,
//! `skeleton_deps::reach_chain_with`, `replan::need_count_with`); the
//! plain ones read `PlanRead::prior()`.

use crate::work_rate::{prior_rate, RateReader};

/// The planner's readers (module doc).
pub struct PlanRead<'a> {
    pub rate: RateReader<'a>,
    pub guide_trains: &'a dyn Fn(&str, u8, u8) -> bool,
}

fn never(_: &str, _: u8, _: u8) -> bool {
    false
}

impl PlanRead<'static> {
    /// The game prior, no guide (offline, tests, a bot without snapshot).
    pub fn prior() -> PlanRead<'static> {
        PlanRead { rate: &prior_rate, guide_trains: &never }
    }
}
