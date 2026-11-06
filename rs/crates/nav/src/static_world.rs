//! Static collision for every level, paged from the server pack.
//!
//! Squares paint on first touch: `get` loads the 64x64 square when it
//! is missing. A square absent from the pack is empty ground, never a
//! wall. Shared between threads through `Arc`, never copied per call.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::collision::{CollisionMap, LocDef, MapPack, StaticLocs, OPEN};

struct PagedState {
    pack: MapPack,
    collision: CollisionMap,
    static_locs: StaticLocs,
    doors: HashMap<(i32, i32, i32), (u16, u8, u8)>,
    toll_gates: HashSet<(i32, i32, i32)>,
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
                doors: HashMap::new(),
                toll_gates: HashSet::new(),
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
        let PagedState { pack, collision, static_locs, doors, toll_gates } = &mut *state;
        pack.ensure_square(collision, static_locs, doors, toll_gates, tile_x >> 6, tile_z >> 6);
        collision.get(tile_x, tile_z, level)
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
