//! Requirement-free detour behind a lifted path (`plan_budgeted_memo`).
//!
//! The verdict search lifts requirements. When its path crosses a door
//! (or custom edge) whose tags the bot lacks, a strict search looks for
//! a longer path that needs none before answering NeedsRequirement.
//!
//! That search used to stop at `detour_cap(lifted)` (2 x lifted + 64)
//! and read the cut as "no detour". The cap holds for short detours
//! only: live 2026-11-15 every route out of the hill giants' hall
//! (3109,9841) answered NeedsRequirement on the brass-key door 1804
//! (alicefield stuck from 03:00, kevin_ellis from 08:46). The worker's
//! danger squares (combat 33: giants and dungeon hunters, +8 a step)
//! price the strict way out (gate 1557/1558, main dungeon, trapdoor
//! ladder) at 1980 to Lumbridge against 453 lifted (cap 970); Varrock ->
//! giants, no danger at all, is 417 strict against 126 lifted (cap 316).
//!
//! Now the capped search only answers fast. Exhausted under the cap,
//! the strict region is sealed: NeedsRequirement. Cut by the cap, each
//! blocked door gets a pocket proof (a strict flood from each side,
//! radius `POCKET_RADIUS`, danger ignored: it changes costs, never
//! connectivity): a side that closes without reaching the other is a
//! pocket the door alone opens (champions' guild), NeedsRequirement
//! again. Otherwise the strict search runs uncapped. A wall-budget cut
//! anywhere is no verdict (`Cut`): the caller answers BudgetExhausted,
//! never cached. Custom edges (boat fares) keep the capped answer: no
//! walk replaces a crossing.
//!
//! Blocked destination (a fishing spot on a pier): the strict search
//! "finds" its closest reachable tile, wherever that is. Such a stand is
//! a detour only when it is as close to the target as the lifted path's
//! (`as_close`); farther, the strict region is exhausted short of the
//! target: Sealed (live 2026-11-17, admin farm:raw_lobster, empty pack
//! at Draynor: the Musa Point pier (2925,3181) planned to the Rimmington
//! shore (2940,3196), read Disconnected without `coins:60`, and the
//! walker never fetched the fare; golden `bateau_sans_piece_ponton`).

#![forbid(unsafe_code)]

use crate::no_route::{detour_cap, DIAGNOSIS_BUDGET};
use crate::plan::{dijkstra, door_stand, EdgeId, NavOptions, Outcome, PrevEdge, SharedEdges, Tile};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Cost radius of the pocket proof flood. A guarded room with its upper
/// floors (champions' guild) closes well inside it; a side that reaches
/// past it is a real area, not a pocket.
pub(crate) const POCKET_RADIUS: u32 = 128;

/// Outcome of the strict detour search.
pub(crate) enum Detour {
    /// A requirement-free path (chain and cost, as `dijkstra` builds it).
    Found(Vec<(Tile, PrevEdge)>, u32),
    /// Proven: no requirement-free path; NeedsRequirement stands.
    Sealed,
    /// The wall budget or the visit cap cut a search: no verdict.
    Cut,
    Aborted,
}

/// Strict search for a requirement-free path after a lifted path of
/// cost `lifted` crossed the `blocked` edges.
#[allow(clippy::too_many_arguments)]
pub(crate) fn strict_detour(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    shared: &SharedEdges,
    lifted: u32,
    lifted_arrival: Tile,
    blocked: &[(EdgeId, Vec<String>)],
    yield_: &mut dyn FnMut() -> bool,
) -> Detour {
    let deadline = std::time::Instant::now() + DIAGNOSIS_BUDGET;
    match dijkstra(world, tables, from, dest, options, false, options.allow_paid, false, shared, detour_cap(lifted), yield_, deadline) {
        Outcome::Aborted => return Detour::Aborted,
        Outcome::Found(chain, cost, arrival) => {
            return if as_close(arrival, lifted_arrival, dest) { Detour::Found(chain, cost) } else { Detour::Sealed };
        }
        Outcome::Closest(_) => return Detour::Sealed,
        Outcome::OverBudget(_) => {}
    }
    let mut doors = Vec::new();
    for (edge, _) in blocked {
        let EdgeId::Door { x, z, level, .. } = edge else {
            return Detour::Sealed;
        };
        let door = Tile { x: *x, z: *z, level: *level };
        let Some((_, shape, angle)) = world.door_at(door.x, door.z, door.level) else {
            return Detour::Sealed;
        };
        let Some(stand) = door_stand(shape, angle, door) else {
            return Detour::Sealed;
        };
        doors.push((door, stand));
    }
    let plain = NavOptions { danger: Vec::new(), ..options.clone() };
    for (door, stand) in doors {
        for (side, other) in [(door, stand), (stand, door)] {
            match dijkstra(world, tables, side, other, &plain, false, options.allow_paid, false, shared, POCKET_RADIUS, yield_, deadline) {
                Outcome::Aborted => return Detour::Aborted,
                Outcome::Closest(_) => return Detour::Sealed,
                Outcome::Found(..) | Outcome::OverBudget(_) => {}
            }
        }
    }
    match dijkstra(world, tables, from, dest, options, false, options.allow_paid, false, shared, u32::MAX, yield_, deadline) {
        Outcome::Aborted => Detour::Aborted,
        Outcome::Found(chain, cost, arrival) if as_close(arrival, lifted_arrival, dest) => Detour::Found(chain, cost),
        Outcome::Found(..) | Outcome::Closest(_) => Detour::Sealed,
        Outcome::OverBudget(_) => Detour::Cut,
    }
}

/// A strict arrival replaces the lifted one only when it stands as close
/// to the target (level, then Chebyshev): an exact destination always
/// does; a blocked one's substitute farther than the lifted stand is the
/// edge of a region the requirement seals, not a detour.
pub(crate) fn as_close(strict: Tile, lifted: Tile, dest: Tile) -> bool {
    let rank = |tile: Tile| ((tile.level - dest.level).abs(), tile.chebyshev(dest));
    rank(strict) <= rank(lifted)
}

#[cfg(test)]
mod tests {
    use super::as_close;
    use crate::plan::Tile;

    /// The Musa Point pier from an empty pack: the Rimmington shore is no
    /// detour for the stand beside the spot; another stand beside it is.
    #[test]
    fn a_far_substitute_is_no_detour() {
        let pier = Tile { x: 2925, z: 3181, level: 0 };
        let lifted = Tile { x: 2924, z: 3180, level: 0 };
        assert!(!as_close(Tile { x: 2940, z: 3196, level: 0 }, lifted, pier), "the shore across the water");
        assert!(as_close(Tile { x: 2926, z: 3180, level: 0 }, lifted, pier), "the other side of the spot");
        assert!(as_close(pier, lifted, pier), "an exact arrival");
        assert!(!as_close(Tile { x: 2925, z: 3181, level: 1 }, lifted, pier), "level first");
    }
}
