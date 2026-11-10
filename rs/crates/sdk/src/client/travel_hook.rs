//! Between-legs travel control (PRD 2, D65).
//!
//! One trip is one plan from the position to the destination (`travel_to`),
//! executed leg by leg. The checks that used to justify cutting the trip
//! into blind 40-tile stages — random event, injury and flight, death —
//! run between two sent legs through one hook, never by changing the
//! destination. The hook observes only; the executor (P1.2) decides:
//! `Continue` walks the next leg, `Fled` replans from the new position,
//! anything else aborts the trip with a typed error.
//!
//! Typed errors: a `NoRoute` on the true destination is `Error::Travel`
//! (never a silent fallback); `is_no_route` is the single predicate the
//! brain matches on. Multi-target distances stay on `Bot::walk_costs`
//! (one call, order-preserving, `None` per unreachable target); the batch
//! wire op it will use is frozen in `lifescape_nav::walk_costs` (N1).

#![forbid(unsafe_code)]

use std::sync::Arc;

use super::Bot;
use crate::error::Error;

/// Where the executor stands between two sent legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegInfo {
    /// Index of the leg just finished (0-based).
    pub seg_index: usize,
    /// Total legs of the plan.
    pub seg_total: usize,
    /// Segment kind just walked: "walk", "door", "ride" or "custom".
    pub kind: &'static str,
}

/// The between-legs verdict: what the executor does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegVerdict {
    /// Walk the next leg.
    Continue,
    /// Random event pending: let the inline rescue run, then resume here.
    RandomEvent,
    /// Injury and flight: replan from the position the bot fled to.
    Fled,
    /// Death: abandon the trip, the caller reports it.
    Dead,
}

/// Control callback the executor runs between two sent legs (D65).
/// Same shape as the pump and pause gates: shared, thread-safe, never
/// pumps — it only reads the bot and returns a verdict.
pub type BetweenLegsHook = Arc<dyn Fn(&mut Bot, &LegInfo) -> LegVerdict + Send + Sync>;

/// True exactly for the typed NoRoute error: `Error::Travel` on the true
/// destination. Every other failure (refused step, shut door, dead ride,
/// timeout, interrupt, io) is a different typed error.
pub fn is_no_route(error: &Error) -> bool {
    matches!(error, Error::Travel(_))
}

/// The frozen NoRoute vocabulary the hook and the executor share.
pub use lifescape_nav::plan::{NoRoute as TravelNoRoute, NoRouteReason as TravelNoRouteReason};

#[cfg(test)]
mod tests {
    use super::{is_no_route, BetweenLegsHook, LegInfo, LegVerdict};
    use crate::error::Error;
    use std::sync::Arc;

    /// The hook type compiles in gate shape and verdicts stay distinct.
    #[test]
    fn hook_verdicts_are_distinct() {
        let hook: BetweenLegsHook = Arc::new(|_, _| LegVerdict::Continue);
        let mut bot = crate::client::Bot::new("127.0.0.1", 43594, ".", ".");
        let info = LegInfo { seg_index: 0, seg_total: 3, kind: "walk" };
        assert_eq!(hook(&mut bot, &info), LegVerdict::Continue);
        assert_ne!(LegVerdict::Continue, LegVerdict::Fled);
        assert_ne!(LegVerdict::RandomEvent, LegVerdict::Dead);
    }

    /// Only `Error::Travel` reads as NoRoute: the typed D65 error.
    #[test]
    fn only_travel_is_no_route() {
        assert!(is_no_route(&Error::Travel("Disconnected".to_string())));
        assert!(!is_no_route(&Error::StepRefused("stall".to_string())));
        assert!(!is_no_route(&Error::Timeout("leg".to_string())));
        assert!(!is_no_route(&Error::Interrupted("stop".to_string())));
    }

    /// `walk_costs` keeps its frozen shape: one batch, order-preserving,
    /// `None` per target. Empty batches cost no remote call.
    #[test]
    fn walk_costs_signature_is_frozen() {
        let mut bot = crate::client::Bot::new("127.0.0.1", 43594, ".", ".");
        assert_eq!(bot.walk_costs(&[]), Vec::<Option<u32>>::new());
    }
}
