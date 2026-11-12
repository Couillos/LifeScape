//! Why a plan failed, and the search budgets whose exhaustion is a
//! reason of its own (`BudgetExhausted`), never a `Disconnected`.
//!
//! Re-exported by `plan` (`plan::NoRoute`, `plan::NoRouteReason`): the
//! public paths stay stable.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::plan::{Blocker, EdgeId, Tile, TransitionRef};

/// Visit cap of one search: explored tiles before giving up. The whole
/// F2P world reachable from Varrock settles in ~239k visits (exhaustive
/// f2p_only miss, measured), the longest legit F2P legs in 80k-183k
/// (Draynor/Falador -> Varrock fountain, Varrock east -> Falador farm):
/// a cut here is `BudgetExhausted`, a finished miss is `Disconnected`.
pub(crate) const MAX_VISITS: usize = 500_000;

/// Wall-clock budget of the verdict search (A). An exhaustive NoRoute
/// once burned ~25 s of worker CPU (four 500k-visit searches back to
/// back); the budget bounds that. The old 3000 ms fit one idle debug
/// thread only: the daemon runs its debug build with 4 pool threads,
/// where the same legs measured (danger squares of an unknown combat
/// level, live load alongside) Falador bank -> fountain 3.7-4.2 s,
/// Draynor bank 4.2-5.1 s, Rimmington 6.2 s, Varrock east -> Falador
/// farm 10.1-11.5 s of search loop (183k visits); release is 5-10x
/// faster. 3000 ms cut them all and the cut read `Disconnected`
/// (george_97-19062, fiona_park-15499). 20 s covers the worst measured
/// leg with ~1.7x headroom and an exhaustive F2P miss (239k visits).
/// Edge setup (`prepare_edges`) is finite and carries no deadline.
pub(crate) const PLAN_BUDGET: Duration = Duration::from_secs(20);

/// Shared wall budget of the diagnosis searches (B tolls, C bounds, D
/// closest) after a miss: they name the price or the fence, B needs
/// ~16k visits on the starved toll case. Fresh, so a long A never eats
/// the toll diagnosis (admin-7526); A + diagnosis stay under the
/// client's `SERVICE_WAIT_MAX` (30 s), or a slow plan reads nav-down.
pub(crate) const DIAGNOSIS_BUDGET: Duration = Duration::from_secs(8);

/// Why no route exists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoRouteReason {
    Disconnected,
    NeedsRequirement,
    PaidOnly,
    OutsideF2p,
    /// The search stopped at a yield point (keepalive or operator stop):
    /// no diagnosis, the caller maps it to `Interrupted`, never a dossier.
    Aborted,
    /// The verdict search hit its budget (`PLAN_BUDGET` wall clock or
    /// `MAX_VISITS`) before reaching the destination or running out of
    /// tiles: nothing is proven, `closest_reachable` is where the cut
    /// left it. Load-dependent, so never cached.
    BudgetExhausted,
}

impl NoRouteReason {
    pub fn as_str(self) -> &'static str {
        match self {
            NoRouteReason::Disconnected => "Disconnected",
            NoRouteReason::NeedsRequirement => "NeedsRequirement",
            NoRouteReason::PaidOnly => "PaidOnly",
            NoRouteReason::OutsideF2p => "OutsideF2p",
            NoRouteReason::Aborted => "Aborted",
            NoRouteReason::BudgetExhausted => "BudgetExhausted",
        }
    }

    /// Wire form back to the reason (`as_str` inverse).
    pub fn parse(raw: &str) -> Result<NoRouteReason, String> {
        match raw {
            "Disconnected" => Ok(NoRouteReason::Disconnected),
            "NeedsRequirement" => Ok(NoRouteReason::NeedsRequirement),
            "PaidOnly" => Ok(NoRouteReason::PaidOnly),
            "OutsideF2p" => Ok(NoRouteReason::OutsideF2p),
            "Aborted" => Ok(NoRouteReason::Aborted),
            "BudgetExhausted" => Ok(NoRouteReason::BudgetExhausted),
            other => Err(format!("NoRoute reason {other:?}")),
        }
    }

    /// A finished verdict, the same for the same key: the service may
    /// cache it. Aborted and BudgetExhausted stopped early and depend on
    /// the moment (operator stop, pool load): the next submit searches.
    pub fn cacheable(self) -> bool {
        !matches!(self, NoRouteReason::Aborted | NoRouteReason::BudgetExhausted)
    }
}

/// A failed plan plus the diagnosis for `replay-nav`.
#[derive(Clone, Debug)]
pub struct NoRoute {
    pub closest_reachable: Tile,
    pub blocked_by_requirements: Vec<(EdgeId, Vec<String>)>,
    pub disabled_transitions_nearby: Vec<TransitionRef>,
    pub reason: NoRouteReason,
    /// Blocking locs near the requested destination, doors first then
    /// nearest (object on the dest tile, sealing doors and walls with
    /// their loc identity): what `Disconnected closest=` never said.
    /// Empty on `Aborted`: the search never ran to a verdict.
    pub blockers: Vec<Blocker>,
}
