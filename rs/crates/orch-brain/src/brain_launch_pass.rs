#![forbid(unsafe_code)]
//! State and flow of one launch pass (lot 2 of the launch refactor).
//! `maybe_launch` is an ordered list of steps over a `LaunchPass`; each
//! step returns a `Flow`: go on, or stop the pass with the way the life
//! is saved (the four ways the old monolithic function returned).

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{LaunchInputs, Life};

use crate::brain::save_life;
use crate::fleet::Fleet;
use crate::jobs::Job;

/// How a stopping step leaves the life on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Save {
    /// Returned without saving.
    None,
    /// Saved (logged on error) only when the pass changed the life.
    IfDirty,
    /// Saved through `save_or_log`, whatever `dirty` says.
    Always,
    /// `let _ = save_life(..)`: saved, an error never logged.
    Silent,
}

/// What a step tells the pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    Stop(Save),
}

/// One launch pass: the handles, the life being decided on, the live
/// snapshot it decides from, the typed inputs and the pass-wide marks.
pub(crate) struct LaunchPass<'a> {
    pub orch: &'a Arc<dyn Fleet>,
    pub job: &'a Arc<Job>,
    pub life: Life,
    pub live: std::sync::Arc<Value>,
    pub inputs: LaunchInputs,
    /// The pass changed the life (saved by the stopping step as asked).
    pub dirty: bool,
    /// The life is a quoting market maker (no ambition, gear, tool or
    /// wish bid, no alchemy).
    pub quoting: bool,
    /// Nodes waiting like a parked buy (market fill, quest, cooled method).
    pub waiting: Vec<String>,
}

impl LaunchPass<'_> {
    /// Applies the save a stopping step asked for.
    pub(crate) fn finish(&self, save: Save) {
        save_as(&self.life, save, self.dirty);
    }
}

/// Writes the life the way `save` says (`dirty`: the pass changed it).
pub(crate) fn save_as(life: &Life, save: Save, dirty: bool) {
    match save {
        Save::None => {}
        Save::IfDirty => {
            if dirty {
                crate::brain_launch::save_or_log(life);
            }
        }
        Save::Always => crate::brain_launch::save_or_log(life),
        Save::Silent => {
            let _ = save_life(life);
        }
    }
}

/// The node a pass picked and what its pre-launch checks carry.
pub(crate) struct Picked {
    /// Index of the node in `life.nodes`.
    pub position: usize,
    /// The resolve context (built after the quest check).
    pub ctx: ReadyCtx,
    /// Coins this bot's other resting buys commit (obtain nodes only).
    pub committed: i64,
}
