//! Static collision for every level, paged from the server pack.
//!
//! Squares paint on first touch: `get` loads the 64x64 square when it
//! is missing. A square absent from the pack is empty ground, never a
//! wall. Shared between threads through `Arc`, never copied per call.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::collision::{CollisionMap, LocDef, MapPack, StaticLayers, StaticLocs, OPEN};

struct PagedState {
    pack: MapPack,
    collision: CollisionMap,
    static_locs: StaticLocs,
    static_layers: StaticLayers,
    doors: HashMap<(i32, i32, i32), (u16, u8, u8)>,
    toll_gates: HashSet<(i32, i32, i32)>,
    f2p: Option<HashSet<(i32, i32)>>,
}

/// Static collision over all levels, backed by the server map pack.
pub struct StaticWorld {
    squares: HashSet<(i32, i32)>,
    state: RwLock<PagedState>,
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
            }),
        }))
    }

    /// True when the pack ships this 64x64 square.
    pub fn has_square(&self, square_x: i32, square_z: i32) -> bool {
        self.squares.contains(&(square_x, square_z))
    }

    /// Collision flags for one tile on one level. Loads the square on
    /// first touch; squares absent from the pack read as empty ground.
    pub fn get(&self, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        if !self.has_square(tile_x >> 6, tile_z >> 6) {
            return OPEN;
        }
        let mut state = self.state.write().expect("static world lock poisoned");
        let PagedState { pack, collision, static_locs, static_layers, doors, toll_gates, .. } = &mut *state;
        pack.ensure_square(collision, static_locs, static_layers, doors, toll_gates, tile_x >> 6, tile_z >> 6);
        collision.get(tile_x, tile_z, level)
    }

    fn ensure(&self, tile_x: i32, tile_z: i32) {
        if !self.has_square(tile_x >> 6, tile_z >> 6) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_world() -> Option<Arc<StaticWorld>> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        if !zip.is_file() {
            return None;
        }
        let gametypes = std::fs::read_to_string(root.join("gamedata/data/gametypes.json")).ok()?;
        let defs = crate::loctypes::parse_loctypes(&gametypes, "{}").ok()?;
        StaticWorld::open(&zip, defs).ok()
    }

    #[test]
    fn lumbridge_open_ground_walks_every_level() {
        let Some(world) = test_world() else { return };
        assert!(crate::collision::tile_walkable(world.get(3091, 3235, 0)));
        for level in 0..4 {
            let _ = world.get(3091, 3235, level);
        }
        assert!(world.has_square(3091 >> 6, 3235 >> 6));
    }

    #[test]
    fn missing_square_reads_empty_never_wall() {
        let Some(world) = test_world() else { return };
        let missing = (0..512)
            .map(|nth| (nth, 907))
            .find(|(square_x, square_z)| !world.has_square(*square_x, *square_z))
            .expect("the pack does not cover every square");
        for level in 0..4 {
            assert_eq!(world.get(missing.0 << 6, missing.1 << 6, level), OPEN);
            assert!(crate::collision::tile_walkable(world.get(missing.0 << 6, missing.1 << 6, level)));
        }
    }

    #[test]
    fn shared_through_arc_without_copy() {
        let Some(world) = test_world() else { return };
        let twin = Arc::clone(&world);
        assert_eq!(world.get(3091, 3235, 0), twin.get(3091, 3235, 0));
    }
}
