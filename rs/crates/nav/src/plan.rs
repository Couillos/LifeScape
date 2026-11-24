//! Multi-level planner: walk plus doors plus transition rides.
//!
//! Dijkstra over tiles (`BinaryHeap`, never A*: dungeon offsets and
//! portals break Chebyshev admissibility). Walk edges reuse the exact
//! server step rule (`bfs::can_step`); door edges remove exactly the
//! closed door's wall (neighbour-aware unpaint, never blanket OPEN);
//! transition edges start on `reach.rs` stands. One function, no
//! network, no bot.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::time::Duration;

use crate::collision::{tile_walkable, BLOCK_EAST, BLOCK_NORTH, BLOCK_SOUTH, BLOCK_WEST};
use crate::reach;
use crate::search::Search;
use crate::static_world::StaticWorld;
use crate::tables::{Tables, Transition, Verdict};

pub use crate::no_route::{NoRoute, NoRouteReason};
pub(crate) use crate::dijkstra::{dijkstra, Outcome};
pub(crate) use crate::no_route::{DIAGNOSIS_BUDGET, MAX_VISITS, PLAN_BUDGET};

/// Identity hash with a bit mixer: tile keys pack level/z/x, so the
/// raw low bits are just the x coordinate and whole columns would
/// share one bucket. The mixer spreads every bit over the key.
#[derive(Default)]
pub(crate) struct IdHash(u64);

impl Hasher for IdHash {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 ^= u64::from_le_bytes(word);
        }
    }

    fn write_u64(&mut self, key: u64) {
        self.0 = key;
    }

    fn finish(&self) -> u64 {
        let mut mixed = self.0.wrapping_add(0x9e3779b97f4a7c15);
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d049bb133111eb);
        mixed ^ (mixed >> 31)
    }
}

pub(crate) type U64Map<T> = HashMap<u64, T, BuildHasherDefault<IdHash>>;

/// LOT3 L2: the shared edge set, computed once per data version and
/// reused by every search on every pool thread (see `service`).
pub(crate) type SharedEdges = (U64Map<Vec<RideEdge>>, U64Map<Vec<CustomPrepared>>);

/// Door crossing: 10 walk steps worth of swinging and stepping.
pub const DOOR_COST: u32 = 10;
/// Open-door mark on a search key: a tile reached through a door click
/// (the door stands open), as opposed to walked onto shut. The mark lives
/// in the top key bit, outside the packed level/z/x, so `unkey` reads the
/// same tile and only `rebuild` must carry the full key. One extra state
/// per door tile: a walked-onto tile is cheap but shut (its click would
/// be a refused src==dest), the clicked tile costs more but steps off.
pub(crate) const FRESH: u64 = 1 << 63;
/// One transition ride: 20 walk steps of climbing and waiting.
pub const TRANSITION_COST: u32 = 20;
/// Riding blind: the prober never saw this transition work.
pub const UNVERIFIED_PENALTY: u32 = 50;

/// One map tile on one level.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tile {
    pub x: i32,
    pub z: i32,
    pub level: i32,
}

impl Tile {
    pub(crate) fn key(self) -> u64 {
        let level = (self.level as u64) & 0xFF;
        let z = ((self.z + 8192) as u64) & 0x1F_FFFF;
        let x = ((self.x + 8192) as u64) & 0x1F_FFFF;
        (level << 42) | (z << 21) | x
    }

    pub(crate) fn chebyshev(self, other: Tile) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }

    /// Rank for the closest-reachable fallback (blocked destination,
    /// no-route diagnosis): level first, so a same-level neighbour
    /// always beats the tile straight above a booth or counter; at equal
    /// Chebyshev a neighbour `walled` off the target (a wall between
    /// them, `fallback::walled_off`) loses to any open one. Exact-destination
    /// search is untouched (dijkstra still settles by cost); only the
    /// fallback/adjust choice changes.
    pub(crate) fn fallback_rank(self, other: Tile, cost: u32, walled: bool) -> (i32, i32, bool, u32) {
        ((self.level - other.level).abs(), self.chebyshev(other), walled, cost)
    }
}

pub(crate) fn unkey(key: u64) -> Tile {
    Tile {
        level: ((key >> 42) & 0xFF) as i32,
        z: (((key >> 21) & 0x1F_FFFF) as i32) - 8192,
        x: ((key & 0x1F_FFFF) as i32) - 8192,
    }
}

/// One loc the planner names in a diagnosis: the object on a blocked
/// destination or a wall/door sealing it. `open_op` is the 1-based door
/// op when the loc swings (`None` on plain walls and furniture);
/// `is_door` is true when this tile is recorded as a swinging door.
/// `stand` is the click tile a straight wall door always accepts (the
/// unconditional arm of the server reach rule) — the actionable side
/// when a pocket seals behind the operator. Other shapes leave it
/// empty: several stands may answer.
#[derive(Clone, Debug)]
pub struct Blocker {
    pub loc: u16,
    pub name: String,
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub open_op: Option<u8>,
    pub is_door: bool,
    pub stand: Option<Tile>,
}

impl Blocker {
    /// One-line human context: which object or door blocks, where it is,
    /// and how to click it (op + stand). Used by every NoRoute display
    /// instead of a bare `closest=`.
    pub fn describe(&self) -> String {
        let kind = if self.is_door { "door" } else { "loc" };
        let mut text = format!(
            "{} '{}' loc={} at ({},{},{})",
            kind, self.name, self.loc, self.x, self.z, self.level
        );
        if let Some(op) = self.open_op {
            text += &format!(" op={op}");
        }
        if let Some(stand) = self.stand {
            text += &format!(" stand=({},{},{})", stand.x, stand.z, stand.level);
        }
        text
    }
}

/// `blockers` rendered for humans, doors first (already ordered upstream).
/// Empty when nothing was identified — never `closest=` alone anymore.
pub fn describe_blockers(blockers: &[Blocker]) -> String {
    blockers.iter().map(Blocker::describe).collect::<Vec<_>>().join("; ")
}

/// Which closed door a step swings through. `open_op`/`toll` ride along
/// for the executor (no pack read worker-side): `None` on old plans
/// means the door stays shut, never clicked blind.
#[derive(Clone, Debug)]
pub struct DoorRef {
    pub loc: u16,
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub open_op: Option<u8>,
    pub toll: bool,
}

/// Which transition row a ride takes.
#[derive(Clone, Debug)]
pub struct TransitionRef {
    pub name: String,
    pub loc: u16,
    pub climb_loc: u16,
    pub op: u8,
    pub from: Tile,
    pub to: Tile,
    /// Every tile the loc covers. Only the origin `from` answers the
    /// climb op (the server finds a loc on its origin tile alone): the
    /// executor and the prober click `from`, never another footprint tile.
    pub footprint: Vec<(i32, i32)>,
    /// Prober verdict carried for the executor message (`NeedsChoice`
    /// names the no-recorded-pick suffix). Absent on old plans: false.
    pub needs_choice: bool,
}

pub use crate::custom_edge::CustomEdgeRef;

/// Opening click before the climb (trapdoor lid, manhole cover...):
/// `loc`/`op` to click, `open_to` the loc id the click must reveal.
#[derive(Clone, Debug)]
pub struct OpenStep {
    pub loc: u16,
    pub op: u8,
    pub open_to: u16,
}

/// One plan piece, in walking order.
#[derive(Clone, Debug)]
pub enum Segment {
    Walk { tiles: Vec<Tile> },
    OpenDoor { door: DoorRef, stand: Tile, cross_to: Tile },
    Ride { transition: TransitionRef, stand: Tile, pre_open: Option<OpenStep>, expect: Tile },
    Custom { edge: CustomEdgeRef, stand: Tile, expect: Tile },
}

/// A full route: segments in order plus their summed edge costs.
#[derive(Clone, Debug)]
pub struct Plan {
    pub segments: Vec<Segment>,
    pub cost: u32,
    /// Set when the destination tile itself is blocked (tree, counter):
    /// the route ends on the closest reachable tile instead.
    pub adjusted_dest: Option<Tile>,
    /// The loc sitting on the requested destination that forced the
    /// adjustment above (`None` on exact plans). Names the object so an
    /// adjustment never reads as a bare coordinate again.
    pub dest_blocker: Option<Blocker>,
}

/// Planner inputs: tolls, F2P bounds, owned tags, banned edges, and the
/// squares where aggressive NPCs would attack this bot.
#[derive(Clone, Debug, Default)]
pub struct NavOptions {
    pub allow_paid: bool,
    pub f2p_only: bool,
    pub satisfied: HashSet<String>,
    pub excluded_edges: HashSet<EdgeId>,
    /// Danger squares (client-computed for the bot's combat level):
    /// walking inside costs `DANGER_STEP_COST` more per step, so routes
    /// go around hunters when a way exists and cross only when none does.
    pub danger: Vec<DangerZone>,
}

/// Danger squares and their lookup live in `danger.rs` (bitmap, CPU).
pub use crate::danger::{DangerMap, DangerZone, DANGER_STEP_COST};

/// One banned edge, for the single executor replan.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum EdgeId {
    Door { loc: u16, x: i32, z: i32, level: i32 },
    Ride(String),
    Custom(String),
}

#[derive(Clone, Debug)]
pub(crate) struct RideEdge {
    pub(crate) index: usize,
    pub(crate) dest: Tile,
    pub(crate) cost: u32,
    pub(crate) pre_open: Option<OpenStep>,
}

#[derive(Clone, Debug)]
pub(crate) struct CustomPrepared {
    pub(crate) index: usize,
    pub(crate) dest: Tile,
    pub(crate) cost: u32,
}

#[derive(Clone, Debug)]
pub(crate) enum PrevEdge {
    Walk,
    Door(DoorRef),
    Ride { index: usize, dest: Tile, pre_open: Option<OpenStep> },
    Custom { index: usize, dest: Tile },
}

/// Plan one route. Pure: same inputs, same plan, byte for byte.
///
/// The first search lifts requirements: its paths are a superset of the
/// fully-filtered ones, so a lifted path using no requirement edge is
/// already the optimal full-filter plan (any better full path would be
/// a better lifted path). Requirement and toll verdicts fall out of the
/// lifted paths without extra full searches.
///
/// When the lifted-optimal path crosses a requirement door (or a
/// custom edge) with missing tags, a strict search (requirements
/// enforced) looks for a longer tag-free detour before answering
/// NeedsRequirement: the brass-key door 1804 has one (the Edgeville
/// trapdoor and the main dungeon reach the same giants' hall).
pub fn plan(world: &StaticWorld, tables: &Tables, from: Tile, dest: Tile, options: &NavOptions) -> Result<Plan, NoRoute> {
    plan_yielding(world, tables, from, dest, options, &mut || true)
}

/// Same search with a yield point every 16384 visits: the live caller
/// pumps its keepalive there (the plan runs on the only socket reader).
/// `false` stops every pending search at once with `Aborted`.
pub fn plan_yielding(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    yield_: &mut dyn FnMut() -> bool,
) -> Result<Plan, NoRoute> {
    if options.f2p_only && !world.in_f2p(dest.x, dest.z) {
        return Err(NoRoute {
            closest_reachable: from,
            blocked_by_requirements: Vec::new(),
            disabled_transitions_nearby: disabled_nearby(tables, dest),
            reason: NoRouteReason::OutsideF2p,
            blockers: seal_nearby(world, tables, dest),
        });
    }
    // LOT1 L2: one permissive prepare_edges per plan, shared by A/B/C/D.
    // Revert: delete this block and `prepare_shared` below, restore the
    // `prepare_edges(&mut search, …)` call in `dijkstra`.
    let setup_start = std::time::Instant::now();
    let shared = match prepare_shared(world, tables, options, yield_) {
        Some(edges) => edges,
        None => return Err(aborted(from)),
    };
    if std::env::var("PLAN_DEBUG").is_ok() {
        eprintln!("plan setup {:?} rides={}", setup_start.elapsed(), shared.0.len());
    }
    plan_yielding_with(world, tables, from, dest, options, &shared, yield_)
}

/// Same search on a precomputed L2 edge set (LOT3: the pool caches one
/// per data version instead of rebuilding it per plan).
pub(crate) fn plan_yielding_with(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    shared: &SharedEdges,
    yield_: &mut dyn FnMut() -> bool,
) -> Result<Plan, NoRoute> {
    plan_budgeted(world, tables, from, dest, options, shared, yield_, PLAN_BUDGET)
}

/// Service path: same plan, sealed-pocket verdicts through the version
/// memo (one flood per destination tile per snapshot, not per plan).
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_memo(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    shared: &SharedEdges,
    memo: &crate::pocket::PocketMemo,
    yield_: &mut dyn FnMut() -> bool,
) -> Result<Plan, NoRoute> {
    plan_budgeted_memo(world, tables, from, dest, options, shared, Some(memo), yield_, PLAN_BUDGET)
}

/// Same plan under an explicit wall budget (tests force a cut with it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_budgeted(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    shared: &SharedEdges,
    yield_: &mut dyn FnMut() -> bool,
    budget: Duration,
) -> Result<Plan, NoRoute> {
    plan_budgeted_memo(world, tables, from, dest, options, shared, None, yield_, budget)
}

#[allow(clippy::too_many_arguments)]
fn plan_budgeted_memo(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    shared: &SharedEdges,
    memo: Option<&crate::pocket::PocketMemo>,
    yield_: &mut dyn FnMut() -> bool,
    budget: Duration,
) -> Result<Plan, NoRoute> {
    // Sealed pocket (walkable, walled in, no edge lands in it): the
    // searches below would sweep the whole world to say Disconnected.
    // The closest names the destination: the verdict is about it, and a
    // closest beside the start reads as a walled-in origin
    // (`brain_stranded`, live tim_thorn-105223 held at the Varrock bank).
    if crate::pocket::sealed_with(memo, world, tables, shared, from, dest) {
        return Err(NoRoute {
            closest_reachable: dest,
            blocked_by_requirements: Vec::new(),
            disabled_transitions_nearby: disabled_nearby(tables, dest),
            reason: NoRouteReason::Disconnected,
            blockers: seal_nearby(world, tables, dest),
        });
    }
    let deadline = std::time::Instant::now() + budget;
    let verdict_cut = match dijkstra(world, tables, from, dest, options, true, options.allow_paid, false, &shared, u32::MAX, yield_, deadline) {
        Outcome::Aborted => return Err(aborted(from)),
        Outcome::Found(chain, cost, best) => {
            let mut blocked = Vec::new();
            for (_, edge) in &chain {
                if let PrevEdge::Door(door) = edge {
                    let missing = tables.missing(door.loc, Tile { x: door.x, z: door.z, level: door.level }, &options.satisfied);
                    if !missing.is_empty() {
                        let blocked_edge = EdgeId::Door { loc: door.loc, x: door.x, z: door.z, level: door.level };
                        if !blocked.iter().any(|(known, _)| *known == blocked_edge) {
                            blocked.push((blocked_edge, missing));
                        }
                    }
                }
                // A hand-added edge with unmet tags (the boat fare not in
                // the pack): the lifted search crossed it, the bot cannot.
                if let PrevEdge::Custom { index, .. } = edge {
                    let custom = &tables.customs[*index];
                    let missing: Vec<String> = custom.requires.iter().filter(|tag| !options.satisfied.contains(*tag)).cloned().collect();
                    let blocked_edge = EdgeId::Custom(custom.name.clone());
                    if !missing.is_empty() && !blocked.iter().any(|(known, _)| *known == blocked_edge) {
                        blocked.push((blocked_edge, missing));
                    }
                }
            }
            if blocked.is_empty() {
                let mut plan = assemble(tables, from, dest, chain, cost);
                if plan.adjusted_dest.is_some() {
                    plan.dest_blocker = blocker_at(world, tables, dest);
                }
                return Ok(plan);
            }
            // The lifted-optimal path crosses a missing requirement: a
            // longer requirement-free detour may still exist (the
            // Edgeville dungeon giants: brass-key door 1804 or the
            // trapdoor and the main dungeon). The strict search finds it;
            // a true pocket (guild door, boat fare) is proven, not assumed
            // from a cost cap (`detour.rs`).
            let reason = match crate::detour::strict_detour(world, tables, from, dest, options, shared, cost, best, &blocked, yield_) {
                crate::detour::Detour::Aborted => return Err(aborted(from)),
                crate::detour::Detour::Found(chain, cost) => {
                    let mut plan = assemble(tables, from, dest, chain, cost);
                    if plan.adjusted_dest.is_some() {
                        plan.dest_blocker = blocker_at(world, tables, dest);
                    }
                    return Ok(plan);
                }
                crate::detour::Detour::Sealed => NoRouteReason::NeedsRequirement,
                crate::detour::Detour::Cut => NoRouteReason::BudgetExhausted,
            };
            return Err(NoRoute {
                closest_reachable: best,
                blocked_by_requirements: blocked,
                disabled_transitions_nearby: disabled_nearby(tables, dest),
                reason,
                blockers: seal_nearby(world, tables, dest),
            });
        }
        Outcome::Closest(_) => None,
        Outcome::OverBudget(best) => Some(best),
    };
    // The miss above may burn its whole wall budget: without a fresh
    // diagnosis budget every follow-up search stops at its first
    // 1024-visit check and the toll diagnosis (PaidOnly) degrades into a
    // bare Disconnected (admin-7526 live). B/C/D share one fresh budget.
    let deadline = std::time::Instant::now() + DIAGNOSIS_BUDGET;
    if !options.allow_paid {
        // B: tolls lifted too. A path here that the searches above lack
        // must swing a toll gate: coins, not walls, block the way.
        match dijkstra(world, tables, from, dest, options, true, true, false, &shared, u32::MAX, yield_, deadline) {
            Outcome::Aborted => return Err(aborted(from)),
            Outcome::Found(chain, _, best) => {
                let mut tolls = Vec::new();
                for (_, edge) in &chain {
                    if let PrevEdge::Door(door) = edge {
                        if world.is_toll(door.x, door.z, door.level) {
                            let toll_edge = EdgeId::Door { loc: door.loc, x: door.x, z: door.z, level: door.level };
                            if !tolls.iter().any(|(known, _)| *known == toll_edge) {
                                tolls.push((toll_edge, vec!["coins:10".to_string()]));
                            }
                        }
                    }
                    // A fare edge (the Port Sarim boat): coins open it too.
                    if let PrevEdge::Custom { index, .. } = edge {
                        let custom = &tables.customs[*index];
                        let fare_edge = EdgeId::Custom(custom.name.clone());
                        if custom.fare > 0 && !tolls.iter().any(|(known, _)| *known == fare_edge) {
                            tolls.push((fare_edge, vec![format!("coins:{}", custom.fare)]));
                        }
                    }
                }
                if !tolls.is_empty() {
                    return Err(NoRoute {
                        closest_reachable: best,
                        blocked_by_requirements: tolls,
                        disabled_transitions_nearby: disabled_nearby(tables, dest),
                        reason: NoRouteReason::PaidOnly,
                        blockers: seal_nearby(world, tables, dest),
                    });
                }
            }
            Outcome::Closest(_) | Outcome::OverBudget(_) => {}
        }
    }
    // A cut proved nothing: a path past the F2P fence (C) or a closest
    // tile (D) would read as a verdict. Only B's toll path, a route that
    // paying opens, outranks the cut.
    if let Some(best) = verdict_cut {
        return Err(NoRoute {
            closest_reachable: best,
            blocked_by_requirements: Vec::new(),
            disabled_transitions_nearby: disabled_nearby(tables, dest),
            reason: NoRouteReason::BudgetExhausted,
            blockers: seal_nearby(world, tables, dest),
        });
    }
    if options.f2p_only {
        // C: bounds lifted, requirements enforced. Reaching the dest this
        // way proves the F2P fence is the blocker.
        match dijkstra(world, tables, from, dest, options, false, options.allow_paid, true, &shared, u32::MAX, yield_, deadline) {
            Outcome::Aborted => return Err(aborted(from)),
            Outcome::Found(_, _, best) => {
                return Err(NoRoute {
                    closest_reachable: best,
                    blocked_by_requirements: Vec::new(),
                    disabled_transitions_nearby: disabled_nearby(tables, dest),
                    reason: NoRouteReason::OutsideF2p,
                    blockers: seal_nearby(world, tables, dest),
                });
            }
            Outcome::Closest(_) | Outcome::OverBudget(_) => {}
        }
    }
    // D: fully filtered search, for the closest reachable tile.
    let closest = match dijkstra(world, tables, from, dest, options, false, options.allow_paid, false, &shared, u32::MAX, yield_, deadline) {
        Outcome::Aborted => return Err(aborted(from)),
        Outcome::Found(_, _, best) | Outcome::Closest(best) | Outcome::OverBudget(best) => best,
    };
    Err(NoRoute {
        closest_reachable: closest,
        blocked_by_requirements: Vec::new(),
        disabled_transitions_nearby: disabled_nearby(tables, dest),
        reason: NoRouteReason::Disconnected,
        blockers: seal_nearby(world, tables, dest),
    })
}

/// No diagnosis: the search never ran to a verdict.
pub(crate) fn aborted_no_route(from: Tile) -> NoRoute {
    aborted(from)
}

fn aborted(from: Tile) -> NoRoute {
    NoRoute {
        closest_reachable: from,
        blocked_by_requirements: Vec::new(),
        disabled_transitions_nearby: Vec::new(),
        reason: NoRouteReason::Aborted,
        blockers: Vec::new(),
    }
}

/// Every click stand of a straight wall door (shape 0), nearest the
/// bot's side first: the door tile itself (walkable stands live there),
/// the unconditional arm of the server reach rule (the exterior side),
/// then the lateral arms whose edge is open in the shut view. Laterals
/// with a shut edge never click (the handler reads "I can't reach
/// that!"); the planner picks the first stand reachable without crossing
/// the shut door, so entry keeps the exterior stand and the Peksa exit
/// clicks from the east (3077,3427, live-proven). Other shapes have no
/// single exterior side: just the tile.
pub fn door_stands(world: &StaticWorld, shape: u8, angle: u8, at: Tile) -> Vec<Tile> {
    let mut stands = vec![at];
    if shape != 0 {
        return stands;
    }
    let (exterior, laterals) = match angle {
        0 => (Tile { x: at.x - 1, z: at.z, level: at.level }, vec![(Tile { x: at.x, z: at.z + 1, level: at.level }, BLOCK_NORTH), (Tile { x: at.x, z: at.z - 1, level: at.level }, BLOCK_SOUTH)]),
        1 => (Tile { x: at.x, z: at.z + 1, level: at.level }, vec![(Tile { x: at.x - 1, z: at.z, level: at.level }, BLOCK_WEST), (Tile { x: at.x + 1, z: at.z, level: at.level }, BLOCK_EAST)]),
        2 => (Tile { x: at.x + 1, z: at.z, level: at.level }, vec![(Tile { x: at.x, z: at.z + 1, level: at.level }, BLOCK_NORTH), (Tile { x: at.x, z: at.z - 1, level: at.level }, BLOCK_SOUTH)]),
        3 => (Tile { x: at.x, z: at.z - 1, level: at.level }, vec![(Tile { x: at.x - 1, z: at.z, level: at.level }, BLOCK_WEST), (Tile { x: at.x + 1, z: at.z, level: at.level }, BLOCK_EAST)]),
        _ => return stands,
    };
    stands.push(exterior);
    for (tile, edge) in laterals {
        if (world.get(tile.x, tile.z, tile.level) & edge) == 0 {
            stands.push(tile);
        }
    }
    stands
}

/// The click tile a straight wall door (shape 0) always accepts under
/// the server reach rule: WEST stands west, NORTH stands north, and so
/// on. Mirrors the unconditional arms in `reach.rs` exactly; other
/// shapes have no single stand, so they report none.
pub(crate) fn door_stand(shape: u8, angle: u8, at: Tile) -> Option<Tile> {
    if shape != 0 {
        return None;
    }
    match angle {
        0 => Some(Tile { x: at.x - 1, z: at.z, level: at.level }),
        1 => Some(Tile { x: at.x, z: at.z + 1, level: at.level }),
        2 => Some(Tile { x: at.x + 1, z: at.z, level: at.level }),
        3 => Some(Tile { x: at.x, z: at.z - 1, level: at.level }),
        _ => None,
    }
}

/// The blocking loc on one tile, if any: the static pick (already the
/// actionable occupant) with its dump name, door op and click side.
fn blocker_at(world: &StaticWorld, tables: &Tables, at: Tile) -> Option<Blocker> {
    let (loc, shape, angle) = world.loc_at(at.x, at.z, at.level)?;
    let def = world.loc_def(loc as usize)?;
    if !def.blockwalk {
        return None;
    }
    let is_door = world.door_at(at.x, at.z, at.level).is_some_and(|(id, _, _)| id == loc);
    Some(Blocker {
        loc,
        name: tables.loc_name(loc).to_string(),
        x: at.x,
        z: at.z,
        level: at.level,
        open_op: def.open_op,
        is_door,
        stand: if is_door { door_stand(shape, angle, at) } else { None },
    })
}

/// Blocking locs in a 25x25 box around the destination, doors first
/// (they swing: loc, op and click side included) then nearest, capped:
/// the object on the dest tile (distance 0) plus the sealing ring.
/// Failure-path only: every tile read may page a map square in.
fn seal_nearby(world: &StaticWorld, tables: &Tables, dest: Tile) -> Vec<Blocker> {
    let mut out = Vec::new();
    for dx in -12..=12 {
        for dz in -12..=12 {
            let at = Tile { x: dest.x + dx, z: dest.z + dz, level: dest.level };
            if let Some(blocker) = blocker_at(world, tables, at) {
                if !out.iter().any(|known: &Blocker| known.x == at.x && known.z == at.z && known.loc == blocker.loc) {
                    out.push(blocker);
                }
            }
        }
    }
    out.sort_by_key(|blocker| (!blocker.is_door, (blocker.x - dest.x).abs() + (blocker.z - dest.z).abs(), blocker.x, blocker.z));
    out.truncate(8);
    out
}

fn disabled_nearby(tables: &Tables, dest: Tile) -> Vec<TransitionRef> {
    tables
        .transitions
        .iter()
        .filter(|row| row.disabled && (row.from.x - dest.x).abs() + (row.from.z - dest.z).abs() <= 40)
        .map(|row| transition_ref(row, tables))
        .collect()
}

fn transition_ref(row: &Transition, tables: &Tables) -> TransitionRef {
    TransitionRef {
        name: row.name.clone(),
        loc: row.loc,
        climb_loc: row.climb_loc,
        op: row.climb_op,
        from: row.from,
        to: row.to,
        footprint: row.footprint.clone(),
        needs_choice: tables
            .verdict(row.loc, row.from)
            .is_some_and(|seen| seen == Verdict::NeedsChoice),
    }
}

/// LOT1 L2: the permissive superset of `prepare_edges` (requires, tolls
/// and F2P bounds all lifted), computed once per `plan()` and shared by
/// the A/B/C/D searches. Lifting can only add usable doors (hence open
/// more stands) and widen bounds, never remove an edge, so each search
/// recovers exactly its own edge set with the stand/dest/requires filter
/// in `dijkstra`. Revert: delete this function (see `plan_yielding`).
pub(crate) fn prepare_shared(
    world: &StaticWorld,
    tables: &Tables,
    options: &NavOptions,
    yield_: &mut dyn FnMut() -> bool,
) -> Option<(U64Map<Vec<RideEdge>>, U64Map<Vec<CustomPrepared>>)> {
    let mut permissive = Search::new(world, tables, options, true, true, true);
    prepare_edges(&mut permissive, yield_)
}

/// Stands per ride: walkable tiles around the loc footprint where the op
/// is valid per `reach.rs` for the footprint, shape and angle. Yields
/// every 16 rows: the setup is finite (one pass over the table) but slow
/// (~0.7 s debug for 367 rows), and with no yield point a slow setup
/// starves the only socket reader — zero keepalive until the loop, so
/// the server reaps the session mid-plan. None is the operator stop
/// (clean Aborted, never a partial edge set: partial edges would plan
/// through missing rides, silently).
fn prepare_edges(search: &mut Search, yield_: &mut dyn FnMut() -> bool) -> Option<(U64Map<Vec<RideEdge>>, U64Map<Vec<CustomPrepared>>)> {
    let mut rides: U64Map<Vec<RideEdge>> = U64Map::default();
    for (index, row) in search.tables.transitions.iter().enumerate() {
        if index % 16 == 0 && !yield_() {
            return None;
        }
        if row.disabled {
            continue;
        }
        if search.tables.verdict(row.loc, row.from).is_some_and(Verdict::excludes) {
            continue;
        }
        let pre_open = match row.open_to {
            Some(open_to) => {
                let op = search.world.loc_def(row.loc as usize).and_then(|def| def.open_op);
                match op {
                    Some(op) => Some(OpenStep { loc: row.loc, op, open_to }),
                    None => continue,
                }
            }
            None => None,
        };
        let cost = TRANSITION_COST
            + if search.tables.verdict(row.loc, row.from).is_some_and(Verdict::verified) {
                0
            } else {
                UNVERIFIED_PENALTY
            };
        for stand in stands(search, row) {
            let dest = match row.delta {
                Some((dl, dx, dz)) => Tile { x: stand.x + dx, z: stand.z + dz, level: stand.level + dl },
                None => row.to,
            };
            if !search.in_bounds(dest) {
                continue;
            }
            rides.entry(stand.key()).or_default().push(RideEdge { index, dest, cost, pre_open: pre_open.clone() });
        }
    }
    let mut customs: U64Map<Vec<CustomPrepared>> = U64Map::default();
    for (index, edge) in search.tables.customs.iter().enumerate() {
        if !search.ignore_requires && !edge.requires.iter().all(|tag| search.options.satisfied.contains(tag)) {
            continue;
        }
        let cost = TRANSITION_COST + UNVERIFIED_PENALTY;
        customs.entry(edge.stand.key()).or_default().push(CustomPrepared { index, dest: edge.to, cost });
    }
    Some((rides, customs))
}

fn stands(search: &mut Search, row: &Transition) -> Vec<Tile> {
    let (mut shape, mut angle) = (10, 0);
    let mut anchor_tiles = row.footprint.clone();
    anchor_tiles.push((row.from.x, row.from.z));
    let mut found = false;
    'loc: for (x, z) in &anchor_tiles {
        for (id, entry_shape, entry_angle) in search.world.locs_at(*x, *z, row.from.level) {
            if id == row.loc {
                shape = entry_shape as i32;
                angle = entry_angle as i32;
                found = true;
                break 'loc;
            }
        }
    }
    if !found {
        // The table tile is not always the answering tile on multi-tile
        // stairs: steer by whatever loc sits on the anchor instead.
        if let Some((_, entry_shape, entry_angle)) = search.world.loc_at(row.from.x, row.from.z, row.from.level) {
            shape = entry_shape as i32;
            angle = entry_angle as i32;
        }
    }
    let (mut width, mut height) = (1, 1);
    // Engine order (`reachedLoc(.., type.width, type.length, ..)`):
    // width spans x unrotated. Swapped, a 2x3 stair at angle 3 read
    // as 2 wide and 3 deep, and a tile beside it "overlapped" the
    // loc (stairs_326 stand north: no_move live 2026-11-13).
    if let Some(def) = search.world.loc_def(row.loc as usize) {
        width = def.width;
        height = def.length;
    }
    let mut candidates: Vec<(i32, i32)> = Vec::new();
    for (x, z) in &anchor_tiles {
        // Reach never needs more than the footprint and its ring: wall,
        // decor and rect stands are all on or beside the loc.
        for dx in -1..=1 {
            for dz in -1..=1 {
                let tile = (*x + dx, *z + dz);
                if !candidates.contains(&tile) {
                    candidates.push(tile);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (x, z) in candidates {
        let stand = Tile { x, z, level: row.from.level };
        if !search.in_bounds(stand) {
            continue;
        }
        if !tile_walkable(search.open(stand)) {
            continue;
        }
        if reach::reached(
            &|ax, az, level| search.world.get(ax, az, level),
            row.from.level,
            x,
            z,
            row.from.x,
            row.from.z,
            width,
            height,
            angle,
            shape,
            row.force_approach,
        ) {
            // A ride landing inside a wall never executes (the server
            // climbs onto a walkable tile): drop the stand here so the
            // prober's first stand — the one verdict the whole row
            // carries — is always a live-climbable one. The old first
            // stand of stairs_316 (west of the stair) landed on a
            // walled L1 tile and its mono-stand no_move verdict shut a
            // working stair.
            let dest = match row.delta {
                Some((dl, dx, dz)) => Tile { x: x + dx, z: z + dz, level: row.from.level + dl },
                None => row.to,
            };
            if !tile_walkable(search.open(dest)) {
                continue;
            }
            out.push(stand);
        }
    }
    // Stand-dependent verdict (stairs_316 live): the probe rode the
    // first stand only, so its Ok covers that stand alone — a
    // geometrically valid sibling (east of the stair) never moved
    // server-side. On delta rows the proven stand replays exactly
    // (proven arrival minus delta), so only it plans. The filter
    // keeps every candidate when the proven stand drops out (rather
    // than bricking the row) and never touches fixed-dest rows.
    if let Some((dl, dx, dz)) = row.delta {
        if let Some(landed) = search.tables.proven_arrival(row.loc, row.from) {
            let proven: Vec<Tile> = out
                .iter()
                .cloned()
                .filter(|stand| {
                    Tile { x: stand.x + dx, z: stand.z + dz, level: stand.level + dl } == landed
                })
                .collect();
            if !proven.is_empty() {
                return proven;
            }
        }
    }
    out
}

/// Deterministic reach-valid stands for one transition row, probe order
/// first. The prober rides from the first element; the planner uses the
/// same list, so one verdict covers every stand edge. Paid doors read
/// open and no requirement tags are held: geometric candidates, of which
/// the planner may use a subset.
pub fn ride_stands(world: &StaticWorld, tables: &Tables, options: &NavOptions, row: &Transition) -> Vec<Tile> {
    let mut search = Search::new(world, tables, options, false, true, false);
    stands(&mut search, row)
}

fn assemble(tables: &Tables, from: Tile, dest: Tile, chain: Vec<(Tile, PrevEdge)>, cost: u32) -> Plan {
    let mut segments = Vec::new();
    let mut walk: Vec<Tile> = Vec::new();
    let mut cursor = from;
    for (at, edge) in chain {
        match edge {
            PrevEdge::Walk => {
                walk.push(at);
                cursor = at;
            }
            PrevEdge::Door(door) => {
                if !walk.is_empty() {
                    segments.push(Segment::Walk { tiles: std::mem::take(&mut walk) });
                }
                // Onto the door tile then off it past the same door reads
                // as one opening, not two.
                let same = matches!(segments.last(), Some(Segment::OpenDoor { door: prev, .. })
                    if prev.loc == door.loc && prev.x == door.x && prev.z == door.z && prev.level == door.level);
                if same {
                    if let Some(Segment::OpenDoor { cross_to, .. }) = segments.last_mut() {
                        *cross_to = at;
                    }
                } else {
                    segments.push(Segment::OpenDoor { stand: cursor, cross_to: at, door });
                }
                cursor = at;
            }
            PrevEdge::Ride { index, dest, pre_open } => {
                if !walk.is_empty() {
                    segments.push(Segment::Walk { tiles: std::mem::take(&mut walk) });
                }
                let row = &tables.transitions[index];
                segments.push(Segment::Ride {
                    stand: cursor,
                    expect: dest,
                    pre_open,
                    transition: transition_ref(row, tables),
                });
                cursor = dest;
            }
            PrevEdge::Custom { index, dest } => {
                if !walk.is_empty() {
                    segments.push(Segment::Walk { tiles: std::mem::take(&mut walk) });
                }
                let edge = &tables.customs[index];
                segments.push(Segment::Custom {
                    edge: CustomEdgeRef::of(edge),
                    stand: cursor,
                    expect: dest,
                });
                cursor = dest;
            }
        }
    }
    if !walk.is_empty() {
        segments.push(Segment::Walk { tiles: walk });
    }
    // Merging happens above: one Walk per run between rides and doors.
    let adjusted_dest = if cursor.key() != dest.key() { Some(cursor) } else { None };
    Plan { segments, cost, adjusted_dest, dest_blocker: None }
}

#[cfg(test)]
mod tests {

    fn real_world() -> (std::sync::Arc<crate::static_world::StaticWorld>, crate::tables::Tables) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
        let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = crate::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        world.set_sky_floors(tables.sky_floors.clone());
        world.set_npc_blockers(tables.npc_blockers.clone());
        (world, tables)
    }

    /// stairs_316 is stand-dependent (admin-5709 live): the probe rode
    /// the south stand to (2971,3373,1), the planner picked the east
    /// stand (2973,3371) and never moved. Only the proven stand plans.
    #[test]
    fn stairs_316_rides_the_proven_south_stand() {
        let (world, tables) = real_world();
        let row = tables.transitions.iter().find(|row| row.name == "stairs_316").expect("stairs_316 row");
        let options = super::NavOptions::default();
        let stands = super::ride_stands(&world, &tables, &options, row);
        assert_eq!(stands, vec![super::Tile { x: 2971, z: 3369, level: 0 }], "only the probed stand plans, got {stands:?}");
    }

    /// End to end on the live dossier inputs: the golden rides south,
    /// never the dead east stand.
    #[test]
    fn stairs_316_golden_plans_through_the_south_stand() {
        let (world, tables) = real_world();
        let from = super::Tile { x: 3222, z: 3218, level: 0 };
        let dest = super::Tile { x: 2970, z: 3376, level: 1 };
        let planned = super::plan(&world, &tables, from, dest, &super::NavOptions::default()).expect("stairs_316 plans");
        let ride = planned
            .segments
            .iter()
            .find_map(|segment| match segment {
                super::Segment::Ride { transition, stand, expect, .. } if transition.name == "stairs_316" => Some((*stand, *expect)),
                _ => None,
            })
            .expect("plan rides stairs_316");
        assert_eq!(ride.0, super::Tile { x: 2971, z: 3369, level: 0 }, "south stand, not the dead east one");
        assert_eq!(ride.1, super::Tile { x: 2971, z: 3373, level: 1 }, "probe landing");
    }

    /// Fallback rank: a same-level neighbour beats the tile straight
    /// above a blocked booth, however cheap the ladder ride is.
    #[test]
    fn fallback_rank_prefers_same_level() {
        use super::Tile;
        let dest = Tile { x: 3186, z: 3446, level: 0 };
        let upstairs = Tile { x: 3186, z: 3446, level: 1 };
        let beside = Tile { x: 3185, z: 3446, level: 0 };
        assert!(beside.fallback_rank(dest, 3, false) < upstairs.fallback_rank(dest, 23, false), "bank booth stays on its level");
    }

    #[test]
    fn describe_blockers_names_door_tile_op_and_stand() {
        let door = super::Blocker {
            loc: 1530,
            name: String::from("Door"),
            x: 3076,
            z: 3427,
            level: 0,
            open_op: Some(1),
            is_door: true,
            stand: Some(super::Tile { x: 3077, z: 3427, level: 0 }),
        };
        let text = super::describe_blockers(std::slice::from_ref(&door));
        assert!(text.contains("door 'Door'"), "kind+nom, got {text}");
        assert!(text.contains("loc=1530"), "loc, got {text}");
        assert!(text.contains("(3076,3427,0)"), "tuile, got {text}");
        assert!(text.contains("op=1"), "op, got {text}");
        assert!(text.contains("stand=(3077,3427,0)"), "stand, got {text}");
        assert_eq!(super::describe_blockers(&[]), "", "vide = pas de bruit");
    }
}

