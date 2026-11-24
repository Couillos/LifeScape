//! Static collision for every level, paged from the server pack.
//!
//! Squares paint on first touch: `get` loads the 64x64 square when it
//! is missing. A square absent from the pack reads as ground on level 0
//! and as void (blocked) above: the server returns NULL for unpainted
//! zones, and upper levels with no painted data are sky, never floor.
//! Shared between threads through `Arc`, never copied per call.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::grid::{Grid, Square};

use crate::collision::{tile_index, zone_index, CollisionMap, LocDef, MapPack, StaticLayers, StaticLocs, NULL, OPEN};

struct PagedState {
    pack: MapPack,
    collision: CollisionMap,
    static_locs: StaticLocs,
    static_layers: StaticLayers,
    doors: HashMap<(i32, i32, i32), (u16, u8, u8)>,
    toll_gates: HashSet<(i32, i32, i32)>,
    f2p: Option<HashSet<(i32, i32)>>,
    sky_floors: HashSet<(i32, i32, i32)>,
    npc_blockers: HashSet<(i32, i32, i32)>,
}

/// Static collision over all levels, backed by the server map pack.
pub struct StaticWorld {
    squares: HashSet<(i32, i32)>,
    state: RwLock<PagedState>,
    /// Dense immutable squares built from `state` (lot N2): swapped for
    /// an empty grid whenever a setter changes what a read answers.
    grid: RwLock<Arc<Grid>>,
}

impl StaticWorld {
    /// Open the world from the stored map pack and the loctype table.
    /// Painting happens lazily through `get`.
    pub fn open(pack_zip: &Path, defs: Vec<Option<LocDef>>) -> std::io::Result<Arc<StaticWorld>> {
        let spans = crate::collision::zip_spans(pack_zip)?;
        let pack = MapPack::new(pack_zip.to_path_buf(), spans, defs);
        let squares = pack.square_list().into_iter().collect();
        Ok(Arc::new(StaticWorld {
            squares,
            state: RwLock::new(PagedState {
                pack,
                collision: CollisionMap::new(),
                static_locs: HashMap::new(),
                static_layers: HashMap::new(),
                doors: HashMap::new(),
                toll_gates: HashSet::new(),
                f2p: None,
                sky_floors: HashSet::new(),
                npc_blockers: HashSet::new(),
            }),
            grid: RwLock::new(Arc::new(Grid::new())),
        }))
    }

    /// 64x64 squares painted so far (lazy paint through `get`): the lot 4
    /// RAM gauge (§5.9 monde line: 1932 squares bound the pack, this counts
    /// how far the warmup has gone).
    pub fn painted_squares(&self) -> usize {
        self.state.read().expect("static world lock poisoned").pack.loaded_count()
    }

    /// True when the pack ships this 64x64 square.
    pub fn has_square(&self, square_x: i32, square_z: i32) -> bool {
        self.squares.contains(&(square_x, square_z))
    }

    /// Collision flags for one tile on one level. Loads the square on
    /// first touch; squares absent from the pack read as ground on level 0
    /// and as void (NULL, blocked) above. Painted-but-empty upper tiles
    /// (flags OPEN with no floor, roof, wall or loc paint) are sky too:
    /// the server leaves them OPEN but no walk ever stands there, so the
    /// planner must not cut through them. `sky_floors` allowlist (real
    /// floors the pack leaves OPEN, proven live) stays OPEN.
    pub fn get(&self, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        match self.grid().square(self, tile_x, tile_z, level) {
            Some(square) => square.flags(tile_x, tile_z),
            None => self.get_paged(tile_x, tile_z, level),
        }
    }

    /// The paged reader `get` answered from before the dense grid (lot
    /// N2), kept as the test oracle and as the fallback for tiles the
    /// grid does not index.
    pub(crate) fn get_paged(&self, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        // Read-lock fast path (CPU 2026-11-14): once a square is painted,
        // every read shares the lock. The write lock (exclusive) is only
        // taken to paint a missing square; it used to be taken on EVERY
        // read, serializing all plan threads on one lock.
        {
            let state = self.state.read().expect("static world lock poisoned");
            if !self.has_square(tile_x >> 6, tile_z >> 6) {
                if level > 0 && state.sky_floors.contains(&(tile_x, tile_z, level)) {
                    return OPEN;
                }
                return if level > 0 { NULL } else { Self::with_blockers(&state, OPEN, tile_x, tile_z, level) };
            }
            if state.pack.is_loaded(tile_x >> 6, tile_z >> 6) {
                return Self::painted_flags(&state, tile_x, tile_z, level);
            }
        }
        let mut state = self.state.write().expect("static world lock poisoned");
        let PagedState { pack, collision, static_locs, static_layers, doors, toll_gates, .. } = &mut *state;
        pack.ensure_square(collision, static_locs, static_layers, doors, toll_gates, tile_x >> 6, tile_z >> 6);
        Self::painted_flags(&state, tile_x, tile_z, level)
    }

    /// Flags of a painted tile (upper OPEN tiles off the allowlist are sky).
    fn painted_flags(state: &PagedState, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        let flags = state.collision.get(tile_x, tile_z, level);
        if level > 0 && flags == OPEN && !state.sky_floors.contains(&(tile_x, tile_z, level)) {
            return NULL;
        }
        Self::with_blockers(state, flags, tile_x, tile_z, level)
    }

    /// A tile a static player-blocking NPC stands on reads as a loc (the
    /// engine sets `BLOCK_NPC_AND_PLAYERS` there at spawn, plan P8).
    fn with_blockers(state: &PagedState, flags: u32, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        if state.npc_blockers.contains(&(tile_x, tile_z, level)) {
            flags | crate::collision::LOC
        } else {
            flags
        }
    }

    /// Tiles of static NPCs that block players (`Tables::npc_blockers`).
    pub fn set_npc_blockers(&self, tiles: HashSet<(i32, i32, i32)>) {
        self.state.write().expect("static world lock poisoned").npc_blockers = tiles;
        self.reset_grid();
    }

    /// The dense grid of this world state (one clone per search).
    pub(crate) fn grid(&self) -> Arc<Grid> {
        Arc::clone(&self.grid.read().expect("grid lock poisoned"))
    }

    /// Drop every built square: a setter changed what a read answers.
    fn reset_grid(&self) {
        *self.grid.write().expect("grid lock poisoned") = Arc::new(Grid::new());
    }

    /// Paint a pack square and its 8 neighbours. A wall loc on a square's
    /// border paints the neighbour's edge tile too, and painting only adds
    /// bits: a square's final flags are known once the 8 around it are
    /// painted. False when the square itself is in the pack and unreadable.
    fn paint_with_neighbours(&self, square_x: i32, square_z: i32) -> bool {
        let wanted: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (square_x + dx, square_z + dz)))
            .filter(|(sx, sz)| self.has_square(*sx, *sz))
            .collect();
        let all_loaded = {
            let state = self.state.read().expect("static world lock poisoned");
            wanted.iter().all(|(sx, sz)| state.pack.is_loaded(*sx, *sz))
        };
        if !all_loaded {
            let mut state = self.state.write().expect("static world lock poisoned");
            let PagedState { pack, collision, static_locs, static_layers, doors, toll_gates, .. } = &mut *state;
            for (sx, sz) in &wanted {
                pack.ensure_square(collision, static_locs, static_layers, doors, toll_gates, *sx, *sz);
            }
        }
        !self.has_square(square_x, square_z)
            || self.state.read().expect("static world lock poisoned").pack.is_loaded(square_x, square_z)
    }

    /// Build the dense square of one 64 x 64 square and level from the
    /// paged state: the same answers `get_paged`, `door_at` and `in_f2p`
    /// give, tile by tile. `None` when its pack square is unreadable
    /// (nothing is cached then, the paged reader retries).
    pub(crate) fn build_square(&self, square_x: i32, square_z: i32, level: i32) -> Option<Arc<Square>> {
        if !self.paint_with_neighbours(square_x, square_z) {
            return None;
        }
        let state = self.state.read().expect("static world lock poisoned");
        let in_pack = self.has_square(square_x, square_z);
        let (base_x, base_z) = (square_x << 6, square_z << 6);
        let mut flags = Box::new([0u32; 64 * 64]);
        if in_pack {
            // Same answer as `painted_flags` per tile, from the zones
            // (8 x 8 tiles each) read once: collision, then the sky rule
            // (an upper OPEN tile off the allowlist is void), then the
            // blocking NPCs.
            let zones = state.collision.raw_zones();
            for zone_z in 0..8 {
                for zone_x in 0..8 {
                    let (zone_base_x, zone_base_z) = (base_x + zone_x * 8, base_z + zone_z * 8);
                    let zone = zones.get(&zone_index(zone_base_x, zone_base_z, level));
                    for dz in 0..8 {
                        for dx in 0..8 {
                            let raw = match zone {
                                Some(words) => words[tile_index(zone_base_x + dx, zone_base_z + dz)],
                                None => NULL,
                            };
                            flags[(((zone_z * 8 + dz) << 6) | (zone_x * 8 + dx)) as usize] = raw;
                        }
                    }
                }
            }
            if level > 0 {
                let mut allowed = [0u64; 64];
                for (floor_x, floor_z, floor_level) in &state.sky_floors {
                    if *floor_level == level && floor_x >> 6 == square_x && floor_z >> 6 == square_z {
                        let slot = (((floor_z & 63) << 6) | (floor_x & 63)) as usize;
                        allowed[slot >> 6] |= 1u64 << (slot & 63);
                    }
                }
                for (slot, value) in flags.iter_mut().enumerate() {
                    if *value == OPEN && allowed[slot >> 6] & (1u64 << (slot & 63)) == 0 {
                        *value = NULL;
                    }
                }
            }
        } else {
            for z in 0..64 {
                for x in 0..64 {
                    let (tile_x, tile_z) = (base_x + x, base_z + z);
                    flags[((z << 6) | x) as usize] = if level > 0 {
                        if state.sky_floors.contains(&(tile_x, tile_z, level)) { OPEN } else { NULL }
                    } else {
                        OPEN
                    };
                }
            }
        }
        for (block_x, block_z, block_level) in &state.npc_blockers {
            if *block_level == level && block_x >> 6 == square_x && block_z >> 6 == square_z {
                // An absent square above ground reads NULL before the
                // blocker rule (`get_paged` returns it without one).
                if !in_pack && level > 0 {
                    continue;
                }
                flags[(((block_z & 63) << 6) | (block_x & 63)) as usize] |= crate::collision::LOC;
            }
        }
        let door_slots: Vec<usize> = state
            .doors
            .keys()
            .filter(|(door_x, door_z, door_level)| *door_level == level && door_x >> 6 == square_x && door_z >> 6 == square_z)
            .map(|(door_x, door_z, _)| (((door_z & 63) << 6) | (door_x & 63)) as usize)
            .collect();
        let f2p = match &state.f2p {
            None => u64::MAX,
            Some(zones) => {
                let mut bits = 0u64;
                for zone_z in 0..8 {
                    for zone_x in 0..8 {
                        if zones.contains(&((square_x << 3) + zone_x, (square_z << 3) + zone_z)) {
                            bits |= 1u64 << ((zone_z << 3) | zone_x);
                        }
                    }
                }
                bits
            }
        };
        Some(Arc::new(Square::new(flags, &door_slots, f2p)))
    }

    /// Pack squares painted so far (N4: the list kept across restarts).
    pub fn painted_square_list(&self) -> Vec<(i32, i32)> {
        let mut list = self.state.read().expect("static world lock poisoned").pack.loaded_list();
        list.sort_unstable();
        list
    }

    /// Paint the given pack squares and build their dense squares on
    /// every level (N4: warm start), so the first searches read a ready
    /// grid. Squares absent from the pack are skipped.
    pub fn warm_squares(&self, list: &[(i32, i32)]) {
        let grid = self.grid();
        for (square_x, square_z) in list {
            if self.has_square(*square_x, *square_z) {
                for level in 0..4 {
                    let _ = grid.square(self, square_x << 6, square_z << 6, level);
                }
            }
        }
    }

    fn ensure(&self, tile_x: i32, tile_z: i32) {
        if !self.has_square(tile_x >> 6, tile_z >> 6) {
            return;
        }
        if self.state.read().expect("static world lock poisoned").pack.is_loaded(tile_x >> 6, tile_z >> 6) {
            return;
        }
        let mut state = self.state.write().expect("static world lock poisoned");
        let PagedState { pack, collision, static_locs, static_layers, doors, toll_gates, .. } = &mut *state;
        pack.ensure_square(collision, static_locs, static_layers, doors, toll_gates, tile_x >> 6, tile_z >> 6);
    }

    /// Closed door recorded at this tile, if any: (loc id, shape, angle).
    pub fn door_at(&self, tile_x: i32, tile_z: i32, level: i32) -> Option<(u16, u8, u8)> {
        self.ensure(tile_x, tile_z);
        self.state.read().expect("static world lock poisoned").doors.get(&(tile_x, tile_z, level)).copied()
    }

    /// Static loc placement at this tile, if any: (loc id, shape, angle).
    pub fn loc_at(&self, tile_x: i32, tile_z: i32, level: i32) -> Option<(u16, u8, u8)> {
        self.ensure(tile_x, tile_z);
        self.state.read().expect("static world lock poisoned").static_locs.get(&(tile_x, tile_z, level)).copied()
    }

    /// Every static loc stacked on this tile, all draw layers:
    /// (loc id, shape, angle). A tile can stack decor over a wall, so the
    /// single `loc_at` pick is not always the blocking one.
    pub fn locs_at(&self, tile_x: i32, tile_z: i32, level: i32) -> Vec<(u16, u8, u8)> {
        self.ensure(tile_x, tile_z);
        let state = self.state.read().expect("static world lock poisoned");
        let mut out = Vec::new();
        for layer in 0..4 {
            if let Some(entry) = state.static_layers.get(&(tile_x, tile_z, level, layer)) {
                out.push(*entry);
            }
        }
        out
    }

    /// Loctype row for one loc id, when the pack knows it.
    pub fn loc_def(&self, loc_id: usize) -> Option<LocDef> {
        self.state.read().expect("static world lock poisoned").pack.loc_def(loc_id).copied()
    }

    /// True when this door tile charges a toll (Al-Kharid gate).
    pub fn is_toll(&self, tile_x: i32, tile_z: i32, level: i32) -> bool {
        self.ensure(tile_x, tile_z);
        self.state.read().expect("static world lock poisoned").toll_gates.contains(&(tile_x, tile_z, level))
    }

    /// Flags with the door recorded here opened: the exact wall removal
    /// (`unpaint_door`), never a blanket OPEN (other walls on the tile
    /// stay). Toll gates stay shut unless paid paths are allowed.
    pub fn open_flag(&self, tile_x: i32, tile_z: i32, level: i32, allow_paid: bool) -> u32 {
        let base = self.get(tile_x, tile_z, level);
        let door = self.door_at(tile_x, tile_z, level);
        let Some((id, shape, angle)) = door else { return base };
        if !allow_paid && self.is_toll(tile_x, tile_z, level) {
            return base;
        }
        let state = self.state.read().expect("static world lock poisoned");
        let mut probe = CollisionMap::new();
        probe.set(tile_x, tile_z, level, base);
        state.pack.unpaint_door(&mut probe, id, shape, angle, tile_x, tile_z, level);
        probe.get(tile_x, tile_z, level)
    }

    /// Remove one door's wall footprint from a caller-owned map. The
    /// scratch map must already hold the surrounding tiles: wall locs
    /// paint shared edges onto their neighbours, so unpainting one tile
    /// alone leaves the neighbour-side bits set.
    pub fn unpaint_into(&self, map: &mut CollisionMap, loc: u16, shape: u8, angle: u8, tile_x: i32, tile_z: i32, level: i32) {
        let state = self.state.read().expect("static world lock poisoned");
        state.pack.unpaint_door(map, loc, shape, angle, tile_x, tile_z, level);
    }

    /// Free-to-play 8x8 zones (`tile >> 3`), parsed from the engine
    /// `free2play.csv`. Unset means no bounds: every tile is inside.
    pub fn set_f2p(&self, zones: HashSet<(i32, i32)>) {
        self.state.write().expect("static world lock poisoned").f2p = Some(zones);
        self.reset_grid();
    }

    /// Upper tiles that stay walkable despite no painted data (real floors
    /// the pack leaves OPEN, proven live). Empty means every upper-OPEN
    /// tile reads as void.
    pub fn set_sky_floors(&self, floors: HashSet<(i32, i32, i32)>) {
        self.state.write().expect("static world lock poisoned").sky_floors = floors;
        self.reset_grid();
    }

    /// True when this tile is on free-to-play ground (level ignored, like
    /// the engine's own check), or when no bounds were set.
    pub fn in_f2p(&self, tile_x: i32, tile_z: i32) -> bool {
        let state = self.state.read().expect("static world lock poisoned");
        match &state.f2p {
            None => true,
            Some(zones) => zones.contains(&(tile_x >> 3, tile_z >> 3)),
        }
    }

    /// Paint every pack square touching the Manhattan diamond (one write
    /// lock for the whole region instead of one per tile). Squares are
    /// self-contained (locs, doors and tolls key inside their own
    /// square), so a region read after this equals the per-tile reads.
    fn ensure_diamond(&self, tile_x: i32, tile_z: i32, radius: i32) {
        let radius = radius.max(0);
        let mut state = self.state.write().expect("static world lock poisoned");
        let PagedState { pack, collision, static_locs, static_layers, doors, toll_gates, .. } = &mut *state;
        for square_x in (tile_x - radius) >> 6..=(tile_x + radius) >> 6 {
            for square_z in (tile_z - radius) >> 6..=(tile_z + radius) >> 6 {
                if self.has_square(square_x, square_z) {
                    pack.ensure_square(collision, static_locs, static_layers, doors, toll_gates, square_x, square_z);
                }
            }
        }
    }

    /// Toll-gate tiles on `level` within a Manhattan radius, nearest
    /// first, ties by x then z: exactly the order a per-tile diamond scan
    /// (x outer, z inner, strict improvement) meets them. One write lock
    /// to paint, one read lock to filter the (tiny) toll set.
    pub fn toll_gates_within(&self, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Vec<(i32, i32, i32)> {
        let radius = radius.max(0);
        self.ensure_diamond(tile_x, tile_z, radius);
        let state = self.state.read().expect("static world lock poisoned");
        let mut gates: Vec<(i32, i32, i32)> = state
            .toll_gates
            .iter()
            .filter(|(gate_x, gate_z, gate_level)| {
                *gate_level == level && (gate_x - tile_x).abs() + (gate_z - tile_z).abs() <= radius
            })
            .copied()
            .collect();
        gates.sort_by_key(|(gate_x, gate_z, _)| ((gate_x - tile_x).abs() + (gate_z - tile_z).abs(), *gate_x, *gate_z));
        gates
    }

    /// Static locs on `level` within a Manhattan radius, in diamond scan
    /// order (x outer, z inner): one write lock to paint, one read lock
    /// for the whole scan (was two locks per tile through `loc_at`).
    pub fn static_locs_within(&self, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Vec<(i32, i32, (u16, u8, u8))> {
        let radius = radius.max(0);
        self.ensure_diamond(tile_x, tile_z, radius);
        let state = self.state.read().expect("static world lock poisoned");
        let mut out = Vec::new();
        for scan_x in tile_x - radius..=tile_x + radius {
            let rest = radius - (scan_x - tile_x).abs();
            for scan_z in tile_z - rest..=tile_z + rest {
                if let Some(entry) = state.static_locs.get(&(scan_x, scan_z, level)) {
                    out.push((scan_x, scan_z, *entry));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_world() -> Option<Arc<StaticWorld>> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        if !zip.is_file() {
            return None;
        }
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).ok()?;
        let defs = crate::loctypes::parse_loctypes(&gametypes, "{}").ok()?;
        StaticWorld::open(&zip, defs).ok()
    }

    #[test]
    fn lumbridge_open_ground_walks_level0_sky_blocked_above() {
        let Some(world) = test_world() else { return };
        assert!(crate::collision::tile_walkable(world.get(3091, 3235, 0)));
        // Open ground on L0, sky above: no painted data on L1-L3, so
        // the planner must see void (server NULL), never walkable sky.
        for level in 1..4 {
            assert_eq!(world.get(3091, 3235, level), crate::collision::NULL);
            assert!(!crate::collision::tile_walkable(world.get(3091, 3235, level)));
        }
        assert!(world.has_square(3091 >> 6, 3235 >> 6));
    }

    #[test]
    fn missing_square_ground_below_void_above() {
        let Some(world) = test_world() else { return };
        let missing = (0..512)
            .map(|nth| (nth, 907))
            .find(|(square_x, square_z)| !world.has_square(*square_x, *square_z))
            .expect("the pack does not cover every square");
        // L0 keeps the historic ground default; above is void like the
        // server (CollisionEngine::get returns NULL for unpainted zones).
        assert_eq!(world.get(missing.0 << 6, missing.1 << 6, 0), OPEN);
        assert!(crate::collision::tile_walkable(world.get(missing.0 << 6, missing.1 << 6, 0)));
        for level in 1..4 {
            assert_eq!(world.get(missing.0 << 6, missing.1 << 6, level), crate::collision::NULL);
            assert!(!crate::collision::tile_walkable(world.get(missing.0 << 6, missing.1 << 6, level)));
        }
    }

    #[test]
    fn shared_through_arc_without_copy() {
        let Some(world) = test_world() else { return };
        let twin = Arc::clone(&world);
        assert_eq!(world.get(3091, 3235, 0), twin.get(3091, 3235, 0));
    }

    #[test]
    fn tmp_dump_grass_flags2() {
        let Some(world) = test_world() else { return };
        for z in (3214..=3224).rev() {
            let mut line = format!("{z:4} ");
            for x in 3218..=3228 {
                line.push_str(&format!("{:08x} ", world.get(x, z, 0)));
            }
            eprintln!("{line}");
        }
        eprintln!("loc_at 3074,3133: {:?}", world.loc_at(3074, 3133, 0));
        eprintln!("loc_at 3070,3132: {:?}", world.loc_at(3070, 3132, 0));
    }
}

