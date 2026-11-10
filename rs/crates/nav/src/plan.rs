//! Multi-level planner: walk plus doors plus transition rides.
//!
//! Dijkstra over tiles (`BinaryHeap`, never A*: dungeon offsets and
//! portals break Chebyshev admissibility). Walk edges reuse the exact
//! server step rule (`bfs::can_step`); door edges remove exactly the
//! closed door's wall (neighbour-aware unpaint, never blanket OPEN);
//! transition edges start on `reach.rs` stands. One function, no
//! network, no bot.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::time::Duration;

use crate::bfs::can_step;
use crate::collision::{tile_walkable, CollisionMap};
use crate::reach;
use crate::static_world::StaticWorld;
use crate::tables::{Tables, Transition, Verdict};

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
/// One transition ride: 20 walk steps of climbing and waiting.
pub const TRANSITION_COST: u32 = 20;
/// Riding blind: the prober never saw this transition work.
pub const UNVERIFIED_PENALTY: u32 = 50;

/// Search budget: explored tiles before calling the area disconnected.
const MAX_VISITS: usize = 500_000;

/// Wall-clock budget for one `plan` call, every search included: an
/// exhaustive NoRoute once burned ~25 s of worker CPU (up to four
/// 500k-visit searches back to back), freezing ticks, snapshots and
/// spectate while the status stayed `running`. Legit plans stay far
/// below (worst golden reachable: 264 tiles in ~1.1 s debug, live legs
/// plan in ~0.2 s); past the budget each pending search stops at its
/// current best, the same Disconnected answer exhaustion would give.
/// Calibrated against the longest legit trek (south Varrock crossing,
/// ~86k visits / ~1.7 s of search loop in debug plus ~0.7 s of edge
/// setup): 3000 ms leaves headroom in debug and stays snappy in
/// release. Edge setup (`prepare_edges`) is finite — one pass over the
/// transition table, it always terminates — so it carries no deadline,
/// only a yield: a slow setup still proves the session alive instead of
/// starving the socket into a server reap, and still honors the
/// operator stop.
const PLAN_BUDGET: Duration = Duration::from_millis(3000);

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
    /// always beats the tile straight above a booth or counter.
    /// Exact-destination search is untouched (dijkstra still settles by
    /// cost); only the fallback/adjust choice changes.
    pub(crate) fn fallback_rank(self, other: Tile, cost: u32) -> (i32, i32, u32) {
        ((self.level - other.level).abs(), self.chebyshev(other), cost)
    }
}

fn unkey(key: u64) -> Tile {
    Tile {
        level: ((key >> 42) & 0xFF) as i32,
        z: (((key >> 21) & 0x1F_FFFF) as i32) - 8192,
        x: ((key & 0x1F_FFFF) as i32) - 8192,
    }
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
    /// Every tile the loc covers: only one of them answers the climb op,
    /// and the table `from` is not always that one. The executor clicks
    /// the footprint tile nearest its stand.
    pub footprint: Vec<(i32, i32)>,
    /// Prober verdict carried for the executor message (`NeedsChoice`
    /// names the no-recorded-pick suffix). Absent on old plans: false.
    pub needs_choice: bool,
}

/// Which hand-added edge a step takes.
#[derive(Clone, Debug)]
pub struct CustomEdgeRef {
    pub name: String,
}

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
}

/// Planner inputs: tolls, F2P bounds, owned tags, banned edges.
#[derive(Clone, Debug, Default)]
pub struct NavOptions {
    pub allow_paid: bool,
    pub f2p_only: bool,
    pub satisfied: HashSet<String>,
    pub excluded_edges: HashSet<EdgeId>,
}

/// One banned edge, for the single executor replan.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum EdgeId {
    Door { loc: u16, x: i32, z: i32, level: i32 },
    Ride(String),
    Custom(String),
}

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
}

impl NoRouteReason {
    pub fn as_str(self) -> &'static str {
        match self {
            NoRouteReason::Disconnected => "Disconnected",
            NoRouteReason::NeedsRequirement => "NeedsRequirement",
            NoRouteReason::PaidOnly => "PaidOnly",
            NoRouteReason::OutsideF2p => "OutsideF2p",
            NoRouteReason::Aborted => "Aborted",
        }
    }
}

/// A failed plan plus the diagnosis for `replay-nav`.
#[derive(Clone, Debug)]
pub struct NoRoute {
    pub closest_reachable: Tile,
    pub blocked_by_requirements: Vec<(EdgeId, Vec<String>)>,
    pub disabled_transitions_nearby: Vec<TransitionRef>,
    pub reason: NoRouteReason,
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
enum PrevEdge {
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
/// Known limit: when the lifted-optimal path crosses a requirement
/// door with missing tags, the answer is NeedsRequirement without
/// searching for a longer tag-free detour. Requirement doors guard
/// enclosed pockets (verified for the single current entry), so no
/// such detour exists; revisit if a requirement door ever gets one.
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
    let deadline = std::time::Instant::now() + PLAN_BUDGET;
    match dijkstra(world, tables, from, dest, options, true, options.allow_paid, false, &shared, yield_, deadline) {
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
            }
            if blocked.is_empty() {
                return Ok(assemble(tables, from, dest, chain, cost));
            }
            return Err(NoRoute {
                closest_reachable: best,
                blocked_by_requirements: blocked,
                disabled_transitions_nearby: disabled_nearby(tables, dest),
                reason: NoRouteReason::NeedsRequirement,
            });
        }
        Outcome::Closest(_) => {}
    }
    if !options.allow_paid {
        // B: tolls lifted too. A path here that the searches above lack
        // must swing a toll gate: coins, not walls, block the way.
        match dijkstra(world, tables, from, dest, options, true, true, false, &shared, yield_, deadline) {
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
                }
                if !tolls.is_empty() {
                    return Err(NoRoute {
                        closest_reachable: best,
                        blocked_by_requirements: tolls,
                        disabled_transitions_nearby: disabled_nearby(tables, dest),
                        reason: NoRouteReason::PaidOnly,
                    });
                }
            }
            Outcome::Closest(_) => {}
        }
    }
    if options.f2p_only {
        // C: bounds lifted, requirements enforced. Reaching the dest this
        // way proves the F2P fence is the blocker.
        match dijkstra(world, tables, from, dest, options, false, options.allow_paid, true, &shared, yield_, deadline) {
            Outcome::Aborted => return Err(aborted(from)),
            Outcome::Found(_, _, best) => {
                return Err(NoRoute {
                    closest_reachable: best,
                    blocked_by_requirements: Vec::new(),
                    disabled_transitions_nearby: disabled_nearby(tables, dest),
                    reason: NoRouteReason::OutsideF2p,
                });
            }
            Outcome::Closest(_) => {}
        }
    }
    // D: fully filtered search, for the closest reachable tile.
    let closest = match dijkstra(world, tables, from, dest, options, false, options.allow_paid, false, &shared, yield_, deadline) {
        Outcome::Aborted => return Err(aborted(from)),
        Outcome::Found(_, _, best) => best,
        Outcome::Closest(best) => best,
    };
    Err(NoRoute {
        closest_reachable: closest,
        blocked_by_requirements: Vec::new(),
        disabled_transitions_nearby: disabled_nearby(tables, dest),
        reason: NoRouteReason::Disconnected,
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
    }
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

enum Outcome {
    Found(Vec<(Tile, PrevEdge)>, u32, Tile),
    Closest(Tile),
    Aborted,
}

struct Search<'a> {
    world: &'a StaticWorld,
    tables: &'a Tables,
    options: &'a NavOptions,
    ignore_requires: bool,
    allow_paid: bool,
    ignore_f2p: bool,
    base_flags: U64Map<u32>,
    open_flags: U64Map<u32>,
    doors: U64Map<Option<(u16, u8, u8)>>,
}

impl Search<'_> {
    fn in_bounds(&self, tile: Tile) -> bool {
        !self.options.f2p_only || self.ignore_f2p || self.world.in_f2p(tile.x, tile.z)
    }

    fn base(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.base_flags.get(&tile.key()) {
            return *flags;
        }
        let flags = self.world.get(tile.x, tile.z, tile.level);
        self.base_flags.insert(tile.key(), flags);
        flags
    }

    fn door_at(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        if let Some(found) = self.doors.get(&tile.key()) {
            return *found;
        }
        let found = self.world.door_at(tile.x, tile.z, tile.level);
        self.doors.insert(tile.key(), found);
        found
    }

    /// A door this search may swing: openable, untolled (or paid), with
    /// its requirement tags in hand, and not hand-blocked tile by tile.
    fn usable_door(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        let (id, shape, angle) = self.door_at(tile)?;
        if self.tables.is_blocked(id, tile) {
            return None;
        }
        let def = self.world.loc_def(id as usize)?;
        def.open_op?;
        if self.world.is_toll(tile.x, tile.z, tile.level) && !self.allow_paid {
            return None;
        }
        if !self.ignore_requires && !self.tables.missing(id, tile, &self.options.satisfied).is_empty() {
            return None;
        }
        Some((id, shape, angle))
    }

    /// Collision with usable doors swung open: the exact wall removal
    /// over the tiles it touches, never a blanket OPEN. Wall locs paint
    /// shared edges onto their neighbours, so the scratch map carries
    /// the 3x3 around the tile and every usable door inside unpaints
    /// into it before the centre tile is read. 3x3 is exact: paint
    /// reaches at most the neighbour tiles, so any door touching the
    /// centre always shows one of its tiles inside the window.
    fn open(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.open_flags.get(&tile.key()) {
            return *flags;
        }
        let mut doors = Vec::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
                if let Some(found) = self.usable_door(near) {
                    doors.push((near, found));
                }
            }
        }
        let flags = if doors.is_empty() {
            self.base(tile)
        } else {
            let mut scratch = CollisionMap::new();
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
                    scratch.set(near.x, near.z, near.level, self.base(near));
                }
            }
            for (near, (id, shape, angle)) in doors {
                self.world.unpaint_into(&mut scratch, id, shape, angle, near.x, near.z, near.level);
            }
            scratch.get(tile.x, tile.z, tile.level)
        };
        self.open_flags.insert(tile.key(), flags);
        flags
    }

    /// Walk step in the planning view (usable doors read open).
    fn step_open(&mut self, from: Tile, to: Tile) -> bool {
        let level = from.level;
        let mut step = |x: i32, z: i32| self.open(Tile { x, z, level });
        can_step(&mut step, from.x, from.z, to.x, to.z)
    }

    /// Walk step with every door shut.
    fn step_closed(&mut self, from: Tile, to: Tile) -> bool {
        let level = from.level;
        let mut step = |x: i32, z: i32| self.base(Tile { x, z, level });
        can_step(&mut step, from.x, from.z, to.x, to.z)
    }
}

/// Search keepalive gate: the visit-count abort stays (operator stop
/// must bite even on a fast search), plus a wall-clock yield so a slow
/// search — live doors, toll detours — still proves the session alive
/// server-side instead of going quiet past the 100-tick reap.
fn yield_due(visits: usize, last_yield: std::time::Instant) -> bool {
    visits % 16384 == 0 || (visits % 16 == 0 && last_yield.elapsed() >= std::time::Duration::from_secs(1))
}

fn dijkstra(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    ignore_requires: bool,
    allow_paid: bool,
    ignore_f2p: bool,
    shared: &SharedEdges,
    yield_: &mut dyn FnMut() -> bool,
    deadline: std::time::Instant,
) -> Outcome {
    let mut search = Search {
        world,
        tables,
        options,
        ignore_requires,
        allow_paid,
        ignore_f2p,
        base_flags: U64Map::default(),
        open_flags: U64Map::default(),
        doors: U64Map::default(),

    };
    let dest_walkable = tile_walkable(search.open(dest));
    let (rides, customs) = shared;
    let mut dist: U64Map<u32> = U64Map::default();
    let mut prev: U64Map<(u64, PrevEdge)> = U64Map::default();
    let mut heap: BinaryHeap<(Reverse<u32>, Reverse<u64>)> = BinaryHeap::new();
    dist.insert(from.key(), 0);
    heap.push((Reverse(0), Reverse(from.key())));
    let mut best = from;
    let mut best_rank = from.fallback_rank(dest, 0);
    let mut visits = 0usize;
    // Wall-clock yield: a huge search (live doors, toll detours) can run
    // seconds without hitting the visit-count yield below, and the live
    // caller only emits its keepalive from the yield callback — a quiet
    // search reads as a dead session server-side (reaped after 100
    // ticks). Yield at most once a second, on top of the count check.
    let mut last_yield = std::time::Instant::now();
    let loop_start = std::time::Instant::now();
    let mut heap_max = 0usize;
    let dest_key = dest.key();
    // Lowest cost first, lowest tile key breaking ties: the same world
    // always yields the same plan.
    while let Some((Reverse(cost), Reverse(key))) = heap.pop() {
        if dist.get(&key).is_some_and(|known| *known != cost) {
            continue;
        }
        visits += 1;
        if yield_due(visits, last_yield) {
            last_yield = std::time::Instant::now();
            if !yield_() {
                return Outcome::Aborted;
            }
        }
        if heap.len() > heap_max {
            heap_max = heap.len();
        }
        if visits > MAX_VISITS {
            break;
        }
        // Shared wall budget (PLAN_BUDGET): an exhaustive miss stops at
        // its current best instead of burning the worker. Same
        // Disconnected answer as visit exhaustion; the operator stop
        // (yield false above) still wins as Aborted.
        if visits % 1024 == 0 && std::time::Instant::now() >= deadline {
            break;
        }
        let here = unkey(key);
        let rank = here.fallback_rank(dest, cost);
        if rank < best_rank {
            best_rank = rank;
            best = here;
        }
        if key == dest_key && dest_walkable {
            if std::env::var("PLAN_DEBUG").is_ok() {
                eprintln!(
                    "plan found visits={visits} heap_max={heap_max} {:?}",
                    loop_start.elapsed(),
                );
            }
            return Outcome::Found(rebuild(&prev, from, dest), cost, dest);
        }
        // Walk and door neighbours, eight directions in fixed order.
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + dx, z: here.z + dz, level: here.level };
            // Blocked destination: the closest tile is the whole answer,
            // so tiles far past the current best can never improve it.
            // The true closest is never pruned: its distance stays under
            // every best seen so far.
            if !dest_walkable && next.level == dest.level && next.chebyshev(dest) > best_rank.1 + 64 {
                continue;
            }
            if !search.in_bounds(next) {
                continue;
            }
            let next_key = next.key();
            // A step onto a usable door tile always swings it: the live
            // server refuses entry onto a closed door from any side, so
            // even an along-the-wall step is a door edge, never a walk.
            if search.usable_door(next).is_none() && search.step_closed(here, next) {
                relax(&mut dist, &mut prev, &mut heap, key, next_key, cost + 1, PrevEdge::Walk);
                continue;
            }
            if !search.step_open(here, next) {
                continue;
            }
            let door_tile = [next, here].into_iter().find(|tile| search.usable_door(*tile).is_some());
            let Some(door_tile) = door_tile else {
                continue;
            };
            let (id, shape, angle) = match search.usable_door(door_tile) {
                Some(found) => found,
                None => continue,
            };
            if here != door_tile && !search.tables.side_ok(id, door_tile, here) {
                continue;
            }
            // Clickable stand only: the engine takes the op from an
            // orthogonal neighbour passing its reach rule for the door's
            // shape and angle (a diagonal or wrong-sided stand reads
            // "I can't reach that!" live). Stepping off the door tile
            // itself (stand == door) is the same opening's second half:
            // the click already happened from the previous stand, which
            // passed this check when stepping on. Flags are the closed
            // view: every nearby door is still shut at click time.
            if (here.x - door_tile.x).abs() + (here.z - door_tile.z).abs() > 1 {
                continue;
            }
            let (width, height) =
                search.world.loc_def(id as usize).map(|def| (def.length, def.width)).unwrap_or((1, 1));
            if (here.x, here.z) != (door_tile.x, door_tile.z) {
                let stand_flag = search.base(here);
                if !reach::reached(
                    &|_, _, _| stand_flag,
                    here.level,
                    here.x,
                    here.z,
                    door_tile.x,
                    door_tile.z,
                    width,
                    height,
                    i32::from(angle),
                    i32::from(shape),
                    0,
                ) {
                    continue;
                }
            }
            let edge = EdgeId::Door { loc: id, x: door_tile.x, z: door_tile.z, level: door_tile.level };
            if options.excluded_edges.contains(&edge) {
                continue;
            }
            relax(
                &mut dist,
                &mut prev,
                &mut heap,
                key,
                next_key,
                cost + DOOR_COST,
                PrevEdge::Door(DoorRef {
                    loc: id,
                    x: door_tile.x,
                    z: door_tile.z,
                    level: door_tile.level,
                    open_op: search.world.loc_def(id as usize).and_then(|def| def.open_op),
                    toll: search.world.is_toll(door_tile.x, door_tile.z, door_tile.level),
                }),
            );
        }
        // Transition and custom rides from this stand. The shared L2 is
        // the permissive superset (requires/tolls/F2P lifted), so a stand
        // this search could not use, or a dest outside its bounds, is
        // skipped here: each search sees exactly the edges its own
        // prepare_edges would have built.
        if let Some(edges) = rides.get(&key) {
            if search.in_bounds(here) && tile_walkable(search.open(here)) {
                for edge in edges {
                    if !search.in_bounds(edge.dest) {
                        continue;
                    }
                    if options.excluded_edges.contains(&EdgeId::Ride(tables.transitions[edge.index].name.clone())) {
                        continue;
                    }
                    let ride = PrevEdge::Ride { index: edge.index, dest: edge.dest, pre_open: edge.pre_open.clone() };
                    relax(&mut dist, &mut prev, &mut heap, key, edge.dest.key(), cost + edge.cost, ride);
                }
            }
        }
        if let Some(edges) = customs.get(&key) {
            for edge in edges {
                if !search.ignore_requires
                    && !search.tables.customs[edge.index].requires.iter().all(|tag| search.options.satisfied.contains(tag))
                {
                    continue;
                }
                if options.excluded_edges.contains(&EdgeId::Custom(tables.customs[edge.index].name.clone())) {
                    continue;
                }
                let custom = PrevEdge::Custom { index: edge.index, dest: edge.dest };
                relax(&mut dist, &mut prev, &mut heap, key, edge.dest.key(), cost + edge.cost, custom);
            }
        }
    }
    // Blocked destination (tree, counter): the closest tile is the plan.
    if !dest_walkable && best.key() != dest_key && dist.contains_key(&best.key()) {
        let cost = dist[&best.key()];
        if std::env::var("PLAN_DEBUG").is_ok() {
            eprintln!("plan adjusted visits={visits} {:?}", loop_start.elapsed());
        }
        let found_best = best;
        return Outcome::Found(rebuild(&prev, from, best), cost, found_best);
    }
    if std::env::var("PLAN_DEBUG").is_ok() {
        eprintln!("plan closest visits={visits} {:?}", loop_start.elapsed());
    }
    Outcome::Closest(best)
}

fn relax(
    dist: &mut U64Map<u32>,
    prev: &mut U64Map<(u64, PrevEdge)>,
    heap: &mut BinaryHeap<(Reverse<u32>, Reverse<u64>)>,
    from_key: u64,
    next_key: u64,
    cost: u32,
    edge: PrevEdge,
) {
    if dist.get(&next_key).is_some_and(|known| *known <= cost) {
        return;
    }
    dist.insert(next_key, cost);
    prev.insert(next_key, (from_key, edge));
    heap.push((Reverse(cost), Reverse(next_key)));
}

fn rebuild(prev: &U64Map<(u64, PrevEdge)>, from: Tile, mut at: Tile) -> Vec<(Tile, PrevEdge)> {
    let mut chain = Vec::new();
    while at.key() != from.key() {
        let Some((parent, _)) = prev.get(&at.key()) else {
            break;
        };
        let parent_tile = unkey(*parent);
        let Some((_, edge)) = prev.get(&at.key()) else {
            break;
        };
        let edge = match edge {
            PrevEdge::Walk => PrevEdge::Walk,
            PrevEdge::Door(door) => PrevEdge::Door(door.clone()),
            PrevEdge::Ride { index, dest, pre_open } => {
                PrevEdge::Ride { index: *index, dest: *dest, pre_open: pre_open.clone() }
            }
            PrevEdge::Custom { index, dest } => PrevEdge::Custom { index: *index, dest: *dest },
        };
        chain.push((at, edge));
        at = parent_tile;
    }
    chain.reverse();
    chain
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
    let mut permissive = Search {
        world,
        tables,
        options,
        ignore_requires: true,
        allow_paid: true,
        ignore_f2p: true,
        base_flags: U64Map::default(),
        open_flags: U64Map::default(),
        doors: U64Map::default(),
    };
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
    if let Some(def) = search.world.loc_def(row.loc as usize) {
        width = def.length;
        height = def.width;
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
            0,
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
    out
}

/// Deterministic reach-valid stands for one transition row, probe order
/// first. The prober rides from the first element; the planner uses the
/// same list, so one verdict covers every stand edge. Paid doors read
/// open and no requirement tags are held: geometric candidates, of which
/// the planner may use a subset.
pub fn ride_stands(world: &StaticWorld, tables: &Tables, options: &NavOptions, row: &Transition) -> Vec<Tile> {
    let mut search = Search {
        world,
        tables,
        options,
        ignore_requires: false,
        allow_paid: true,
        ignore_f2p: false,
        base_flags: U64Map::default(),
        open_flags: U64Map::default(),
        doors: U64Map::default(),
    };
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
                    edge: CustomEdgeRef { name: edge.name.clone() },
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
    Plan { segments, cost, adjusted_dest }
}

#[cfg(test)]
mod tests {
    use super::yield_due;
    use std::time::{Duration, Instant};

    /// The keepalive gate: count abort every 16384 visits, wall-clock
    /// yield every 16 visits past one second, silence otherwise.
    #[test]
    fn yield_gate_count_and_clock() {
        let fresh = Instant::now();
        let old = Instant::now() - Duration::from_secs(2);
        assert!(yield_due(16384, fresh), "count abort bites even on a fast search");
        assert!(yield_due(2048, old), "slow search yields past one second");
        assert!(!yield_due(2048, fresh), "no yield mid-second");
        assert!(!yield_due(100, old), "no yield off the visit grid");
        assert!(!yield_due(17, old), "no yield off the 16-visit grid");
    }

    /// Fallback rank: a same-level neighbour beats the tile straight
    /// above a blocked booth, however cheap the ladder ride is.
    #[test]
    fn fallback_rank_prefers_same_level() {
        use super::Tile;
        let dest = Tile { x: 3186, z: 3446, level: 0 };
        let upstairs = Tile { x: 3186, z: 3446, level: 1 };
        let beside = Tile { x: 3185, z: 3446, level: 0 };
        assert!(beside.fallback_rank(dest, 3) < upstairs.fallback_rank(dest, 23), "bank booth stays on its level");
    }
}
