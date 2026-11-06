//! Collision flags and map loading, mirroring the server routefinder.
//!
//! Ports the packed-map loaders: all flag constants, [`CollisionMap`]
//! with its wall/loc painters, [`step_open`], and map loading with the
//! same source priority as the server tooling (`.cache/maps-server.zip`
//! first, `server/maps/` directory fallback). The zip holds stored
//! (uncompressed) entries, so a small std-only reader suffices.
//! JSON loc types stay in the gamedata crate.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use super::buffer::{ReadError, Reader};

pub const OPEN: u32 = 0x0;
pub const WALL_NORTH_WEST: u32 = 0x1;
pub const WALL_NORTH: u32 = 0x2;
pub const WALL_NORTH_EAST: u32 = 0x4;
pub const WALL_EAST: u32 = 0x8;
pub const WALL_SOUTH_EAST: u32 = 0x10;
pub const WALL_SOUTH: u32 = 0x20;
pub const WALL_SOUTH_WEST: u32 = 0x40;
pub const WALL_WEST: u32 = 0x80;
pub const LOC: u32 = 0x100;
pub const WALL_NORTH_WEST_PROJ: u32 = 0x200;
pub const WALL_NORTH_PROJ: u32 = 0x400;
pub const WALL_NORTH_EAST_PROJ: u32 = 0x800;
pub const WALL_EAST_PROJ: u32 = 0x1000;
pub const WALL_SOUTH_EAST_PROJ: u32 = 0x2000;
pub const WALL_SOUTH_PROJ: u32 = 0x4000;
pub const WALL_SOUTH_WEST_PROJ: u32 = 0x8000;
pub const WALL_WEST_PROJ: u32 = 0x10000;
pub const LOC_PROJ_BLOCKER: u32 = 0x20000;
pub const FLOOR_DECORATION: u32 = 0x40000;
pub const NPC_OCC: u32 = 0x80000;
pub const BLOCK_NPC_AND_PLAYERS: u32 = 0x100000;
pub const FLOOR: u32 = 0x200000;
pub const PLAYER_OCC: u32 = 0x400000;
pub const ROOF: u32 = 0x80000000;
pub const FLOOR_BLOCKED: u32 = 0x240000;
pub const WALK_BLOCKED: u32 = 0x240100;
pub const BLOCK_WEST: u32 = 0x240108;
pub const BLOCK_EAST: u32 = 0x240180;
pub const BLOCK_SOUTH: u32 = 0x240102;
pub const BLOCK_NORTH: u32 = 0x240120;
pub const BLOCK_SOUTH_WEST: u32 = 0x24010E;
pub const BLOCK_SOUTH_EAST: u32 = 0x240183;
pub const BLOCK_NORTH_WEST: u32 = 0x240138;
pub const BLOCK_NORTH_EAST: u32 = 0x2401E0;

/// Sentinel for tiles in map squares never loaded.
pub const NULL: u32 = 0x7FFFFFFF;
const MASK32: u32 = 0xFFFFFFFF;

pub const WALL_STRAIGHT: u8 = 0;
pub const WALL_DIAGONAL_CORNER: u8 = 1;
pub const WALL_L: u8 = 2;
pub const WALL_SQUARE_CORNER: u8 = 3;

pub const ANGLE_WEST: u8 = 0;
pub const ANGLE_NORTH: u8 = 1;
pub const ANGLE_EAST: u8 = 2;
pub const ANGLE_SOUTH: u8 = 3;

pub const LAYER_WALL: u8 = 0;
pub const LAYER_WALL_DECOR: u8 = 1;
pub const LAYER_GROUND: u8 = 2;
pub const LAYER_GROUND_DECOR: u8 = 3;

pub const BLOCK_MAP_SQUARE: u8 = 0x1;
pub const LINK_BELOW: u8 = 0x2;
pub const REMOVE_ROOFS: u8 = 0x4;

pub const MAP_SIZE: usize = 64;
pub const LEVELS: usize = 4;
pub const LANDS: usize = MAP_SIZE * MAP_SIZE * LEVELS;

/// Minimal loc-type view the map loader needs (JSON parsing stays in gamedata).
#[derive(Debug, Clone, Copy)]
pub struct LocDef {
    pub blockwalk: bool,
    pub blockrange: bool,
    pub length: i32,
    pub width: i32,
    pub active: i32,
    pub has_ops: bool,
    /// 1-based op index of the "open" action, when the loc can be opened.
    pub open_op: Option<u8>,
    /// The "open" action charges a toll (Al-Kharid gate): never routed
    /// through unless the session explicitly allows paid paths.
    pub toll: bool,
}

/// True unless every direction is blocked (roof/floor flags alone are fine).
pub fn tile_walkable(flags: u32) -> bool {
    !((flags & BLOCK_NORTH) != 0
        && (flags & BLOCK_EAST) != 0
        && (flags & BLOCK_SOUTH) != 0
        && (flags & BLOCK_WEST) != 0)
}

pub fn zone_index(tile_x: i32, tile_z: i32, level: i32) -> u32 {
    (((tile_x >> 3) & 0x7FF) | (((tile_z >> 3) & 0x7FF) << 11) | ((level & 0x3) << 22)) as u32
}

pub fn tile_index(tile_x: i32, tile_z: i32) -> usize {
    ((tile_x & 0x7) | ((tile_z & 0x7) << 3)) as usize
}

pub fn pack_land(land_x: i32, land_z: i32, level: i32) -> usize {
    ((land_z & 0x3F) | ((land_x & 0x3F) << 6) | ((level & 0x3) << 12)) as usize
}

/// Read view over collision flags; defined in [`crate::bfs`], re-exported
/// here so map builders and planning overlays share one import path.
pub use crate::bfs::CollisionSource;

/// 8x8-tile zones keyed like the server, flags painted by locs and walls.
#[derive(Debug, Default, Clone)]
pub struct CollisionMap {
    zones: HashMap<u32, [u32; 64]>,
}

impl CollisionMap {
    pub fn new() -> Self {
        Self::default()
    }

    fn zone_mut(&mut self, tile_x: i32, tile_z: i32, level: i32) -> &mut [u32; 64] {
        self.zones.entry(zone_index(tile_x, tile_z, level)).or_insert([0; 64])
    }

    pub fn get(&self, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        match self.zones.get(&zone_index(tile_x, tile_z, level)) {
            Some(zone) => zone[tile_index(tile_x, tile_z)],
            None => NULL,
        }
    }

    /// True when this tile belongs to a map zone actually loaded.
    pub fn has(&self, tile_x: i32, tile_z: i32, level: i32) -> bool {
        self.zones.contains_key(&zone_index(tile_x, tile_z, level))
    }

    /// Zone flags keyed like the server. Offline tools walk them with
    /// unsigned 8x8 bases, exactly like the packed format iteration.
    pub fn raw_zones(&self) -> &HashMap<u32, [u32; 64]> {
        &self.zones
    }

    /// Every tile carrying any flag, used to read a painted door footprint.
    pub fn flagged_tiles(&self) -> Vec<(i32, i32, i32)> {
        // Inverse of zone_index, with sign extension for negative squares.
        fn zone_base(field: u32) -> i32 {
            let signed = field as i32;
            (if signed >= 0x400 { signed - 0x800 } else { signed }) << 3
        }
        let mut tiles = Vec::new();
        for (zone_key, zone) in &self.zones {
            let base_x = zone_base(zone_key & 0x7FF);
            let base_z = zone_base((zone_key >> 11) & 0x7FF);
            let level = ((zone_key >> 22) & 0x3) as i32;
            for (slot, flags) in zone.iter().enumerate() {
                if *flags != OPEN {
                    let slot = slot as i32;
                    tiles.push((base_x + (slot & 0x7), base_z + ((slot >> 3) & 0x7), level));
                }
            }
        }
        tiles
    }

    pub fn is_flagged(&self, tile_x: i32, tile_z: i32, level: i32, mask: u32) -> bool {
        self.get(tile_x, tile_z, level) & mask != OPEN
    }

    pub fn set(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32) {
        let slot = tile_index(tile_x, tile_z);
        self.zone_mut(tile_x, tile_z, level)[slot] = mask & MASK32;
    }

    pub fn add(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32) {
        let slot = tile_index(tile_x, tile_z);
        let zone = self.zone_mut(tile_x, tile_z, level);
        zone[slot] = (zone[slot] | mask) & MASK32;
    }

    pub fn remove(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32) {
        let slot = tile_index(tile_x, tile_z);
        let zone = self.zone_mut(tile_x, tile_z, level);
        zone[slot] = (zone[slot] & !mask) & MASK32;
    }

    pub fn change_floor(&mut self, tile_x: i32, tile_z: i32, level: i32, add: bool) {
        if add {
            self.add(tile_x, tile_z, level, FLOOR);
        } else {
            self.remove(tile_x, tile_z, level, FLOOR);
        }
    }

    pub fn change_roof(&mut self, tile_x: i32, tile_z: i32, level: i32, add: bool) {
        if add {
            self.add(tile_x, tile_z, level, ROOF);
        } else {
            self.remove(tile_x, tile_z, level, ROOF);
        }
    }

    pub fn change_npc(&mut self, tile_x: i32, tile_z: i32, level: i32, size: i32, add: bool) {
        self.square(tile_x, tile_z, level, size, NPC_OCC, add);
    }

    pub fn change_block(&mut self, tile_x: i32, tile_z: i32, level: i32, size: i32, add: bool) {
        self.square(tile_x, tile_z, level, size, BLOCK_NPC_AND_PLAYERS, add);
    }

    pub fn change_player_occ(&mut self, tile_x: i32, tile_z: i32, level: i32, size: i32, add: bool) {
        self.square(tile_x, tile_z, level, size, PLAYER_OCC, add);
    }

    fn square(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        size: i32,
        mask: u32,
        add: bool,
    ) {
        for index in 0..size * size {
            let dest_x = tile_x + index % size;
            let dest_z = tile_z + index / size;
            if add {
                self.add(dest_x, dest_z, level, mask);
            } else {
                self.remove(dest_x, dest_z, level, mask);
            }
        }
    }

    pub fn change_loc(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        width: i32,
        length: i32,
        blockrange: bool,
        add: bool,
    ) {
        let mut mask = LOC;
        if blockrange {
            mask |= LOC_PROJ_BLOCKER;
        }
        for index in 0..width * length {
            let dest_x = tile_x + index % width;
            let dest_z = tile_z + index / width;
            if add {
                self.add(dest_x, dest_z, level, mask);
            } else {
                self.remove(dest_x, dest_z, level, mask);
            }
        }
    }

    pub fn change_wall(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        shape: u8,
        blockrange: bool,
        add: bool,
    ) {
        if shape == WALL_STRAIGHT {
            self.wall_straight(tile_x, tile_z, level, angle, blockrange, add);
        } else if shape == WALL_DIAGONAL_CORNER || shape == WALL_SQUARE_CORNER {
            self.wall_corner(tile_x, tile_z, level, angle, blockrange, add);
        } else if shape == WALL_L {
            self.wall_l(tile_x, tile_z, level, angle, blockrange, add);
        }
    }

    fn wall_mask(normal: u32, projectile: u32, blockrange: bool) -> u32 {
        if blockrange { projectile } else { normal }
    }

    fn wall_straight(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        blockrange: bool,
        add: bool,
    ) {
        let west = Self::wall_mask(WALL_WEST, WALL_WEST_PROJ, blockrange);
        let east = Self::wall_mask(WALL_EAST, WALL_EAST_PROJ, blockrange);
        let north = Self::wall_mask(WALL_NORTH, WALL_NORTH_PROJ, blockrange);
        let south = Self::wall_mask(WALL_SOUTH, WALL_SOUTH_PROJ, blockrange);
        match angle {
            ANGLE_WEST => self.wall_pair(tile_x, tile_z, level, west, tile_x - 1, tile_z, level, east, add),
            ANGLE_NORTH => self.wall_pair(tile_x, tile_z, level, north, tile_x, tile_z + 1, level, south, add),
            ANGLE_EAST => self.wall_pair(tile_x, tile_z, level, east, tile_x + 1, tile_z, level, west, add),
            ANGLE_SOUTH => self.wall_pair(tile_x, tile_z, level, south, tile_x, tile_z - 1, level, north, add),
            _ => {}
        }
        if blockrange {
            self.wall_straight(tile_x, tile_z, level, angle, false, add);
        }
    }

    fn wall_corner(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        blockrange: bool,
        add: bool,
    ) {
        let north_west = Self::wall_mask(WALL_NORTH_WEST, WALL_NORTH_WEST_PROJ, blockrange);
        let south_east = Self::wall_mask(WALL_SOUTH_EAST, WALL_SOUTH_EAST_PROJ, blockrange);
        let north_east = Self::wall_mask(WALL_NORTH_EAST, WALL_NORTH_EAST_PROJ, blockrange);
        let south_west = Self::wall_mask(WALL_SOUTH_WEST, WALL_SOUTH_WEST_PROJ, blockrange);
        match angle {
            ANGLE_WEST => self.wall_pair(
                tile_x, tile_z, level, north_west, tile_x - 1, tile_z + 1, level, south_east, add,
            ),
            ANGLE_NORTH => self.wall_pair(
                tile_x, tile_z, level, north_east, tile_x + 1, tile_z + 1, level, south_west, add,
            ),
            ANGLE_EAST => self.wall_pair(
                tile_x, tile_z, level, south_east, tile_x + 1, tile_z - 1, level, north_west, add,
            ),
            ANGLE_SOUTH => self.wall_pair(
                tile_x, tile_z, level, south_west, tile_x - 1, tile_z - 1, level, north_east, add,
            ),
            _ => {}
        }
        if blockrange {
            self.wall_corner(tile_x, tile_z, level, angle, false, add);
        }
    }

    fn wall_l(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        blockrange: bool,
        add: bool,
    ) {
        let west = Self::wall_mask(WALL_WEST, WALL_WEST_PROJ, blockrange);
        let east = Self::wall_mask(WALL_EAST, WALL_EAST_PROJ, blockrange);
        let north = Self::wall_mask(WALL_NORTH, WALL_NORTH_PROJ, blockrange);
        let south = Self::wall_mask(WALL_SOUTH, WALL_SOUTH_PROJ, blockrange);
        match angle {
            ANGLE_WEST => {
                self.wall_single(tile_x, tile_z, level, north | west, add);
                self.wall_single(tile_x - 1, tile_z, level, east, add);
                self.wall_single(tile_x, tile_z + 1, level, south, add);
            }
            ANGLE_NORTH => {
                self.wall_single(tile_x, tile_z, level, north | east, add);
                self.wall_single(tile_x + 1, tile_z, level, west, add);
                self.wall_single(tile_x, tile_z + 1, level, south, add);
            }
            ANGLE_EAST => {
                self.wall_single(tile_x, tile_z, level, south | east, add);
                self.wall_single(tile_x + 1, tile_z, level, west, add);
                self.wall_single(tile_x, tile_z - 1, level, north, add);
            }
            ANGLE_SOUTH => {
                self.wall_single(tile_x, tile_z, level, south | west, add);
                self.wall_single(tile_x, tile_z - 1, level, north, add);
                self.wall_single(tile_x - 1, tile_z, level, east, add);
            }
            _ => {}
        }
        if blockrange {
            self.wall_l(tile_x, tile_z, level, angle, false, add);
        }
    }

    fn wall_pair(
        &mut self,
        first_x: i32,
        first_z: i32,
        first_level: i32,
        first_mask: u32,
        second_x: i32,
        second_z: i32,
        second_level: i32,
        second_mask: u32,
        add: bool,
    ) {
        self.wall_single(first_x, first_z, first_level, first_mask, add);
        self.wall_single(second_x, second_z, second_level, second_mask, add);
    }

    fn wall_single(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32, add: bool) {
        if add {
            self.add(tile_x, tile_z, level, mask);
        } else {
            self.remove(tile_x, tile_z, level, mask);
        }
    }
}

pub fn layer_of(shape: u8) -> u8 {
    if shape <= 3 {
        LAYER_WALL
    } else if (4..=8).contains(&shape) {
        LAYER_WALL_DECOR
    } else if shape == 22 {
        LAYER_GROUND_DECOR
    } else {
        LAYER_GROUND
    }
}

pub fn change_loc_collision(
    collision: &mut CollisionMap,
    shape: u8,
    angle: u8,
    blockrange: bool,
    length: i32,
    width: i32,
    active: i32,
    tile_x: i32,
    tile_z: i32,
    level: i32,
    add: bool,
) {
    match layer_of(shape) {
        LAYER_WALL => collision.change_wall(tile_x, tile_z, level, angle, shape, blockrange, add),
        LAYER_GROUND => {
            if angle == ANGLE_NORTH || angle == ANGLE_SOUTH {
                collision.change_loc(tile_x, tile_z, level, length, width, blockrange, add);
            } else {
                collision.change_loc(tile_x, tile_z, level, width, length, blockrange, add);
            }
        }
        LAYER_GROUND_DECOR => {
            if active == 1 {
                collision.change_floor(tile_x, tile_z, level, add);
            }
        }
        _ => {}
    }
}

/// Priority for the single-slot static table: actionable/blocking locs
/// (doors, centre statue) win over stacked ground decor (maze dug soil).
fn score(def: Option<LocDef>) -> u8 {
    match def {
        None => 0,
        Some(entry) => u8::from(entry.blockwalk) * 2 + u8::from(entry.has_ops),
    }
}

/// True when a size-1 mover can step one tile straight (diagonals rejected:
/// only the four cardinals are single steps).
pub fn step_open(
    collision: &CollisionMap,
    tile_x: i32,
    tile_z: i32,
    level: i32,
    dest_dx: i32,
    dest_dz: i32,
) -> bool {
    let here = collision.get(tile_x, tile_z, level);
    let there = collision.get(tile_x + dest_dx, tile_z + dest_dz, level);
    if !tile_walkable(here) || !tile_walkable(there) {
        return false;
    }
    if dest_dx == 0 && dest_dz == 1 {
        return here & WALL_NORTH == 0 && there & WALL_SOUTH == 0;
    }
    if dest_dx == 0 && dest_dz == -1 {
        return here & WALL_SOUTH == 0 && there & WALL_NORTH == 0;
    }
    if dest_dx == 1 && dest_dz == 0 {
        return here & WALL_EAST == 0 && there & WALL_WEST == 0;
    }
    if dest_dx == -1 && dest_dz == 0 {
        return here & WALL_WEST == 0 && there & WALL_EAST == 0;
    }
    false
}

/// Loaded static tables: one loc per tile, the actionable subset, and one
/// entry per draw layer (a ground decor must not hide a wall).
pub type StaticLocs = HashMap<(i32, i32, i32), (u16, u8, u8)>;
pub type StaticLayers = HashMap<(i32, i32, i32, u8), (u16, u8, u8)>;

/// Full map load: collision plus static tables.
pub fn load_maps(
    pack_dir: &Path,
    loctypes: &[Option<LocDef>],
) -> io::Result<(CollisionMap, StaticLocs, StaticLocs, StaticLayers)> {
    let mut collision = CollisionMap::new();
    let mut static_locs = StaticLocs::new();
    let mut static_actions = StaticLocs::new();
    let mut static_layers = StaticLayers::new();
    for (square_x, square_z, ground, locs) in map_files(pack_dir)? {
        paint_square(
            Some(&mut collision),
            &ground,
            &locs,
            square_x,
            square_z,
            loctypes,
            &mut static_locs,
            &mut static_actions,
            &mut static_layers,
        )
        .map_err(to_io)?;
    }
    Ok((collision, static_locs, static_actions, static_layers))
}

/// Paint one 64x64 map square (ground flags plus locs) into the
/// accumulated tables. Powers per-square lazy loading in the worker.
#[allow(clippy::too_many_arguments)]
pub fn paint_square(
    mut collision: Option<&mut CollisionMap>,
    ground: &[u8],
    locs: &[u8],
    square_x: i32,
    square_z: i32,
    loctypes: &[Option<LocDef>],
    static_locs: &mut StaticLocs,
    static_actions: &mut StaticLocs,
    static_layers: &mut StaticLayers,
) -> Result<(), ReadError> {
    let lands = read_lands(ground)?;
    if let Some(map) = collision.as_mut() {
        load_ground(map, &lands, square_x, square_z);
    }
    load_locs(
        collision,
        &lands,
        locs,
        square_x,
        square_z,
        loctypes,
        static_locs,
        static_actions,
        static_layers,
    )
}

/// Static locs without building the collision map (spectator path).
pub fn load_static_locs(
    pack_dir: &Path,
    loctypes: &[Option<LocDef>],
) -> io::Result<(StaticLocs, StaticLocs, StaticLayers)> {
    let mut static_locs = StaticLocs::new();
    let mut static_actions = StaticLocs::new();
    let mut static_layers = StaticLayers::new();
    for (square_x, square_z, ground, locs) in map_files(pack_dir)? {
        paint_square(
            None,
            &ground,
            &locs,
            square_x,
            square_z,
            loctypes,
            &mut static_locs,
            &mut static_actions,
            &mut static_layers,
        )
        .map_err(to_io)?;
    }
    Ok((static_locs, static_actions, static_layers))
}

fn map_files(pack_dir: &Path) -> io::Result<Vec<(i32, i32, Vec<u8>, Vec<u8>)>> {
    let zipped = pack_dir.join(".cache").join("maps-server.zip");
    if zipped.is_file() {
        return map_files_zip(&zipped);
    }
    map_files_dir(&pack_dir.join("server").join("maps"))
}

/// Stored-only zip reader: the engine pack ships every map entry
/// uncompressed, so no inflater is needed. Returns the raw bytes of every
/// entry whose name starts with `m` (ground) alongside its `l` (locs) twin.
fn map_files_zip(zip_path: &Path) -> io::Result<Vec<(i32, i32, Vec<u8>, Vec<u8>)>> {
    let bytes = fs::read(zip_path)?;
    let entries = zip_index(&bytes).map_err(|detail| io::Error::new(io::ErrorKind::InvalidData, detail))?;
    let mut squares: Vec<(i32, i32)> = Vec::new();
    for name in entries.keys() {
        if let Some(coords) = name.strip_prefix('m') {
            let mut parts = coords.split('_');
            if let (Some(mx), Some(mz), None) = (parts.next(), parts.next(), parts.next()) {
                if let (Ok(mx), Ok(mz)) = (mx.parse::<i32>(), mz.parse::<i32>()) {
                    squares.push((mx, mz));
                }
            }
        }
    }
    squares.sort_unstable();
    let mut out = Vec::with_capacity(squares.len());
    for (mx, mz) in squares {
        let ground_name = format!("m{mx}_{mz}");
        let loc_name = format!("l{mx}_{mz}");
        let ground = entries.get(&ground_name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("zip lacks {ground_name}")))?;
        let locs = entries.get(&loc_name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("zip lacks {loc_name}")))?;
        out.push((mx << 6, mz << 6, zip_slice(&bytes, *ground).to_vec(), zip_slice(&bytes, *locs).to_vec()));
    }
    Ok(out)
}

/// Central-directory scan of a zip with stored entries only.
/// One stored entry's location inside the zip blob (no copy).
#[derive(Clone, Copy, Debug)]
pub struct ZipSpan {
    pub offset: usize,
    pub len: usize,
}

/// Central-directory scan of a stored-only zip: entry name -> data span.
/// The pack ships every map uncompressed, so slicing the blob suffices.
pub fn zip_index(bytes: &[u8]) -> Result<HashMap<String, ZipSpan>, String> {
    let tail = bytes.len().saturating_sub(65557 + 22);
    let eocd = bytes[tail..]
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .map(|pos| tail + pos)
        .ok_or_else(|| "zip has no end-of-central-directory".to_string())?;
    let get_u16 = |at: usize| -> Result<usize, String> {
        bytes.get(at..at + 2).and_then(|pair| Some(u16::from_le_bytes([pair[0], pair[1]]) as usize)).ok_or_else(|| "zip truncated".to_string())
    };
    let get_u32 = |at: usize| -> Result<usize, String> {
        bytes.get(at..at + 4).and_then(|quad| Some(u32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]]) as usize)).ok_or_else(|| "zip truncated".to_string())
    };
    let dir_count = get_u16(eocd + 10)?;
    let dir_offset = get_u32(eocd + 16)?;
    let mut entries = HashMap::new();
    let mut pos = dir_offset;
    for _ in 0..dir_count {
        if bytes.get(pos..pos + 4) != Some(b"PK\x01\x02".as_slice()) {
            return Err("zip central directory corrupt".to_string());
        }
        let method = get_u16(pos + 10)?;
        let size = get_u32(pos + 24)?;
        let name_len = get_u16(pos + 28)?;
        let extra_len = get_u16(pos + 30)?;
        let comment_len = get_u16(pos + 32)?;
        let local_offset = get_u32(pos + 42)?;
        let name = bytes.get(pos + 46..pos + 46 + name_len).ok_or_else(|| "zip truncated".to_string())?;
        let name = std::str::from_utf8(name).map_err(|_| "zip name is not UTF-8".to_string())?.to_string();
        pos += 46 + name_len + extra_len + comment_len;
        if method != 0 {
            return Err(format!("zip entry {name} is compressed, stored-only reader cannot take it"));
        }
        let local_name_len = get_u16(local_offset + 26)?;
        let local_extra_len = get_u16(local_offset + 28)?;
        let data_at = local_offset + 30 + local_name_len + local_extra_len;
        if bytes.get(data_at..data_at + size).is_none() {
            return Err(format!("zip entry {name} runs past the end"));
        }
        entries.insert(name, ZipSpan { offset: data_at, len: size });
    }
    Ok(entries)
}

/// Slice one stored entry out of the zip blob.
pub fn zip_slice<'blob>(bytes: &'blob [u8], span: ZipSpan) -> &'blob [u8] {
    &bytes[span.offset..span.offset + span.len]
}

/// One map pack shared by every session through the OS page cache.
/// Workers read squares straight out of the pack file (`read_at`, no copy
/// kept): per-session memory is only the spans index, the loc defs and
/// the painted working set.
pub struct MapPack {
    zip_path: std::path::PathBuf,
    squares: HashMap<(i32, i32), (FileSpan, FileSpan)>,
    defs: Vec<Option<LocDef>>,
    loaded: HashSet<(i32, i32)>,
}

/// Byte range of one stored zip entry inside the pack file.
#[derive(Clone, Copy, Debug)]
pub struct FileSpan {
    pub offset: u64,
    pub len: usize,
}

impl MapPack {
    pub fn new(zip_path: std::path::PathBuf, spans: HashMap<String, FileSpan>, defs: Vec<Option<LocDef>>) -> MapPack {
        MapPack { zip_path, squares: scan_squares(&spans), defs, loaded: HashSet::new() }
    }

    pub fn square_count(&self) -> usize {
        self.squares.len()
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    pub fn loc_def(&self, loc_id: usize) -> Option<&LocDef> {
        self.defs.get(loc_id).and_then(|slot| slot.as_ref())
    }

    /// Planning view: the base map with every known door read open.
    /// The walker clicks the doors it must cross for real; BFS just needs
    /// to see through them.
    /// Planning view: the base map with known doors read open. Toll gates
    /// stay shut unless paid paths are allowed: their whole point is to
    /// cost coins, never the default route.
    pub fn planning_view(&self, base: &CollisionMap, doors: &HashMap<(i32, i32, i32), (u16, u8, u8)>, toll_gates: &HashSet<(i32, i32, i32)>, allow_paid: bool) -> CollisionMap {
        let mut view = base.clone();
        for ((x, z, level), (id, shape, angle)) in doors {
            if !allow_paid && toll_gates.contains(&(*x, *z, *level)) {
                continue;
            }
            self.unpaint_door(&mut view, *id, *shape, *angle, *x, *z, *level);
        }
        view
    }

    /// Undo one loc's collision footprint (door opened): exact inverse of
    /// what `paint_square` laid down for it.
    pub fn unpaint_door(&self, collision: &mut CollisionMap, loc_id: u16, shape: u8, angle: u8, x: i32, z: i32, level: i32) {
        if let Some(def) = self.loc_def(loc_id as usize) {
            change_loc_collision(collision, shape, angle, def.blockrange, def.length, def.width, def.active, x, z, level, false);
        }
    }

    /// Record openable blocking locs painted in one square as doors.
    fn record_square(
        &self,
        static_locs: &StaticLocs,
        doors: &mut HashMap<(i32, i32, i32), (u16, u8, u8)>,
        toll_gates: &mut HashSet<(i32, i32, i32)>,
        square_x: i32,
        square_z: i32,
    ) {
        for x in (square_x << 6)..(square_x << 6) + 64 {
            for z in (square_z << 6)..(square_z << 6) + 64 {
                for level in 0..4 {
                    if let Some((id, shape, angle)) = static_locs.get(&(x, z, level)) {
                        let openable = self.loc_def(*id as usize).is_some_and(|def| def.open_op.is_some() && def.blockwalk);
                        if !openable {
                            continue;
                        }
                        doors.insert((x, z, level), (*id, *shape, *angle));
                        let toll = self.loc_def(*id as usize).is_some_and(|def| def.toll);
                        if toll {
                            toll_gates.insert((x, z, level));
                        }
                    }
                }
            }
        }
    }

    fn read_span(&self, span: FileSpan) -> std::io::Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let file = std::fs::File::open(&self.zip_path)?;
        let mut out = vec![0u8; span.len];
        let mut read = 0;
        while read < span.len {
            let got = file.read_at(&mut out[read..], span.offset + read as u64)?;
            if got == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "pack shrank underfoot"));
            }
            read += got;
        }
        Ok(out)
    }

    /// All indexed squares (for explicit full-map tools).
    pub fn square_list(&self) -> Vec<(i32, i32)> {
        self.squares.keys().copied().collect()
    }

    /// Paint one square by its map coords. False when missing or unreadable.
    pub fn ensure_square(
        &mut self,
        collision: &mut CollisionMap,
        static_locs: &mut StaticLocs,
        doors: &mut HashMap<(i32, i32, i32), (u16, u8, u8)>,
        toll_gates: &mut HashSet<(i32, i32, i32)>,
        square_x: i32,
        square_z: i32,
    ) -> bool {
        if !self.loaded.insert((square_x, square_z)) {
            return true;
        }
        let Some((ground_span, locs_span)) = self.squares.get(&(square_x, square_z)) else {
            return true;
        };
        let (Ok(ground), Ok(locs)) = (self.read_span(*ground_span), self.read_span(*locs_span)) else {
            self.loaded.remove(&(square_x, square_z));
            return false;
        };
        let mut actions = StaticLocs::new();
        let mut layers = StaticLayers::new();
        if paint_square(
            Some(collision),
            &ground,
            &locs,
            square_x << 6,
            square_z << 6,
            &self.defs,
            static_locs,
            &mut actions,
            &mut layers,
        )
        .is_err()
        {
            self.loaded.remove(&(square_x, square_z));
            return false;
        }
        self.record_square(static_locs, doors, toll_gates, square_x, square_z);
        true
    }

    /// Paint every square in the index (explicit full-map tools only:
    /// dumps and checks. Gameplay uses `ensure_area` and stays local).
    pub fn ensure_all(
        &mut self,
        collision: &mut CollisionMap,
        static_locs: &mut StaticLocs,
        doors: &mut HashMap<(i32, i32, i32), (u16, u8, u8)>,
        toll_gates: &mut HashSet<(i32, i32, i32)>,
    ) {
        let squares = self.square_list();
        for (square_x, square_z) in squares {
            self.ensure_square(collision, static_locs, doors, toll_gates, square_x, square_z);
        }
    }

    /// Paint every not-yet-loaded 64x64 square touching the radius. Missing
    /// squares (open void) count as loaded and read as blocked.
    pub fn ensure_area(
        &mut self,
        collision: &mut CollisionMap,
        static_locs: &mut StaticLocs,
        doors: &mut HashMap<(i32, i32, i32), (u16, u8, u8)>,
        toll_gates: &mut HashSet<(i32, i32, i32)>,
        x: i32,
        z: i32,
        radius: i32,
    ) {
        for square_x in (x - radius) >> 6..=(x + radius) >> 6 {
            for square_z in (z - radius) >> 6..=(z + radius) >> 6 {
                if !self.loaded.insert((square_x, square_z)) {
                    continue;
                }
                let Some((ground_span, locs_span)) = self.squares.get(&(square_x, square_z)) else {
                    continue;
                };
                let (Ok(ground), Ok(locs)) = (self.read_span(*ground_span), self.read_span(*locs_span)) else {
                    self.loaded.remove(&(square_x, square_z));
                    continue;
                };
                let mut actions = StaticLocs::new();
                let mut layers = StaticLayers::new();
                if paint_square(
                    Some(collision),
                    &ground,
                    &locs,
                    square_x << 6,
                    square_z << 6,
                    &self.defs,
                    static_locs,
                    &mut actions,
                    &mut layers,
                )
                .is_err()
                {
                    self.loaded.remove(&(square_x, square_z));
                    continue;
                }
                self.record_square(static_locs, doors, toll_gates, square_x, square_z);
            }
        }
    }
}

/// Index every stored entry of the pack file by name, without loading it.
pub fn zip_spans(zip_path: &std::path::Path) -> std::io::Result<HashMap<String, FileSpan>> {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::open(zip_path)?;
    let size = file.metadata()?.len();
    let probe = size.min(65557 + 22) as usize;
    let mut tail = vec![0u8; probe];
    {
        let mut read = 0;
        while read < probe {
            let got = file.read_at(&mut tail[read..], size - probe as u64 + read as u64)?;
            if got == 0 {
                break;
            }
            read += got;
        }
        tail.truncate(read);
    }
    let base = size - tail.len() as u64;
    let eocd = tail
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .map(|pos| base as usize + pos);
    let eocd = eocd.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "zip has no end-of-central-directory"))?;
    let get_u16 = |at: usize| -> std::io::Result<usize> {
        let mut pair = [0u8; 2];
        file.read_at(&mut pair, eocd as u64 + at as u64)?;
        Ok(u16::from_le_bytes(pair) as usize)
    };
    let get_u32 = |at: usize| -> std::io::Result<u64> {
        let mut quad = [0u8; 4];
        file.read_at(&mut quad, eocd as u64 + at as u64)?;
        Ok(u32::from_le_bytes(quad) as u64)
    };
    let dir_count = get_u16(10)?;
    let dir_offset = get_u32(16)?;
    let mut entries = HashMap::new();
    let mut pos = dir_offset;
    for _ in 0..dir_count {
        let mut magic = [0u8; 4];
        file.read_at(&mut magic, pos)?;
        if magic != *b"PK\x01\x02" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "zip central directory corrupt"));
        }
        let mut fixed = [0u8; 42];
        {
            let mut read = 0;
            while read < 42 {
                let got = file.read_at(&mut fixed[read..], pos + 4 + read as u64)?;
                if got == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "zip truncated"));
                }
                read += got;
            }
        }
        let method = u16::from_le_bytes([fixed[6], fixed[7]]);
        let size = u32::from_le_bytes([fixed[20], fixed[21], fixed[22], fixed[23]]) as usize;
        let name_len = u16::from_le_bytes([fixed[24], fixed[25]]) as usize;
        let extra_len = u16::from_le_bytes([fixed[26], fixed[27]]) as usize;
        let comment_len = u16::from_le_bytes([fixed[28], fixed[29]]) as usize;
        let local_offset = u32::from_le_bytes([fixed[38], fixed[39], fixed[40], fixed[41]]) as u64;
        let mut name_raw = vec![0u8; name_len];
        {
            let mut read = 0;
            while read < name_len {
                let got = file.read_at(&mut name_raw[read..], pos + 46 + read as u64)?;
                if got == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "zip truncated"));
                }
                read += got;
            }
        }
        pos += 46 + name_len as u64 + extra_len as u64 + comment_len as u64;
        if method != 0 {
            continue;
        }
        let name = String::from_utf8(name_raw)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "zip name is not UTF-8"))?;
        let mut local = [0u8; 30];
        {
            let mut read = 0;
            while read < 30 {
                let got = file.read_at(&mut local[read..], local_offset + read as u64)?;
                if got == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "zip truncated"));
                }
                read += got;
            }
        }
        let local_name_len = u16::from_le_bytes([local[26], local[27]]) as u64;
        let local_extra_len = u16::from_le_bytes([local[28], local[29]]) as u64;
        let data_at = local_offset + 30 + local_name_len + local_extra_len;
        entries.insert(name, FileSpan { offset: data_at, len: size });
    }
    Ok(entries)
}

fn scan_squares(index: &HashMap<String, FileSpan>) -> HashMap<(i32, i32), (FileSpan, FileSpan)> {
    let mut out = HashMap::new();
    for name in index.keys() {
        let Some(coords) = name.strip_prefix('m') else { continue };
        let mut parts = coords.split('_');
        if let (Some(mx), Some(mz), None) = (parts.next(), parts.next(), parts.next()) {
            if let (Ok(mx), Ok(mz)) = (mx.parse::<i32>(), mz.parse::<i32>()) {
                let ground = format!("m{mx}_{mz}");
                let locs = format!("l{mx}_{mz}");
                if let (Some(ground), Some(locs)) = (index.get(&ground), index.get(&locs)) {
                    out.insert((mx, mz), (*ground, *locs));
                }
            }
        }
    }
    out
}

fn map_files_dir(maps_dir: &Path) -> io::Result<Vec<(i32, i32, Vec<u8>, Vec<u8>)>> {
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(&maps_dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.starts_with('m') {
            names.push(name);
        }
    }
    names.sort();
    let mut squares = Vec::with_capacity(names.len());
    for name in names {
        let coords: Vec<&str> = name[1..].split('_').collect();
        if coords.len() != 2 {
            continue;
        }
        let square_mx: i32 = coords[0].parse().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, format!("bad map name: {name}"))
        })?;
        let square_mz: i32 = coords[1].parse().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, format!("bad map name: {name}"))
        })?;
        let ground = fs::read(maps_dir.join(&name))?;
        let loc_name = format!("l{square_mx}_{square_mz}");
        let locs = fs::read(maps_dir.join(loc_name))?;
        squares.push((square_mx << 6, square_mz << 6, ground, locs));
    }
    Ok(squares)
}

fn to_io(err: ReadError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

fn read_lands(ground: &[u8]) -> Result<Vec<u8>, ReadError> {
    let mut lands = vec![0u8; LANDS];
    let mut packet = Reader::new(ground);
    for level in 0..LEVELS {
        for land_x in 0..MAP_SIZE {
            for land_z in 0..MAP_SIZE {
                loop {
                    let opcode = packet.u8()?;
                    if opcode == 0 {
                        break;
                    }
                    if opcode == 1 {
                        packet.pos += 1;
                        break;
                    }
                    if opcode <= 49 {
                        packet.pos += 1;
                    } else if opcode <= 81 {
                        lands[pack_land(land_x as i32, land_z as i32, level as i32)] = opcode - 49;
                    }
                }
            }
        }
    }
    Ok(lands)
}

fn load_ground(collision: &mut CollisionMap, lands: &[u8], square_x: i32, square_z: i32) {
    for level in 0..LEVELS as i32 {
        for land_x in 0..MAP_SIZE as i32 {
            let absolute_x = land_x + square_x;
            for land_z in 0..MAP_SIZE as i32 {
                let absolute_z = land_z + square_z;
                if land_x % 7 == 0 && land_z % 7 == 0 {
                    collision.zone_mut(absolute_x, absolute_z, level);
                }
                let land = lands[pack_land(land_x, land_z, level)];
                if land & REMOVE_ROOFS != 0 {
                    collision.change_roof(absolute_x, absolute_z, level, true);
                }
                if land & BLOCK_MAP_SQUARE == 0 {
                    continue;
                }
                let bridged = if level == 1 {
                    lands[pack_land(land_x, land_z, level)] & LINK_BELOW
                } else {
                    lands[pack_land(land_x, land_z, 1)] & LINK_BELOW
                };
                let actual = if bridged != 0 { level - 1 } else { level };
                if actual < 0 {
                    continue;
                }
                collision.change_floor(absolute_x, absolute_z, actual, true);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_locs(
    mut collision: Option<&mut CollisionMap>,
    lands: &[u8],
    locs: &[u8],
    square_x: i32,
    square_z: i32,
    loctypes: &[Option<LocDef>],
    static_locs: &mut StaticLocs,
    static_actions: &mut StaticLocs,
    static_layers: &mut StaticLayers,
) -> Result<(), ReadError> {
    let mut packet = Reader::new(locs);
    let mut loc_id: i32 = -1;
    let mut offset = packet.smarts()?;
    while offset != 0 {
        loc_id += i32::from(offset);
        let mut coord: i32 = 0;
        let mut coord_offset = packet.smarts()?;
        while coord_offset != 0 {
            coord += i32::from(coord_offset) - 1;
            let loc_z = coord & 0x3F;
            let loc_x = (coord >> 6) & 0x3F;
            let level = (coord >> 12) & 0x3;
            let info = packet.u8()?;
            coord_offset = packet.smarts()?;
            let absolute_x = loc_x + square_x;
            let absolute_z = loc_z + square_z;
            let bridged = if level == 1 {
                lands[coord as usize] & LINK_BELOW
            } else {
                lands[pack_land(loc_x, loc_z, 1)] & LINK_BELOW
            };
            let actual = if bridged != 0 { level - 1 } else { level };
            if actual < 0 {
                continue;
            }
            let shape = info >> 2;
            let angle = info & 0x3;
            // One tile can stack several locs (maze walls under dug soil):
            // keep the actionable/blocking one so find_loc and the door
            // table still see the centre statue and the walllow doors.
            let key = (absolute_x, absolute_z, actual);
            let newcomer = (loc_id as u16, shape, angle);
            let newcomer_score = score(loctypes.get(loc_id as usize).and_then(|slot| *slot));
            let keep = match static_locs.get(&key) {
                None => true,
                Some((old_id, _, _)) => {
                    newcomer_score
                        > score(loctypes.get(*old_id as usize).and_then(|slot| *slot))
                }
            };
            if keep {
                static_locs.insert(key, newcomer);
            }
            static_layers.insert(
                (absolute_x, absolute_z, actual, layer_of(shape)),
                (loc_id as u16, shape, angle),
            );
            let loc = loctypes.get(loc_id as usize).and_then(|slot| *slot);
            if loc.is_some_and(|def| def.has_ops) {
                static_actions
                    .entry((absolute_x, absolute_z, actual))
                    .or_insert((loc_id as u16, shape, angle));
            }
            let map_slot = collision.as_mut().map(|slot| &mut **slot);
            if let (Some(map), Some(def)) = (map_slot, loc) {
                if def.blockwalk {
                    change_loc_collision(
                        map,
                        shape,
                        angle,
                        def.blockrange,
                        def.length,
                        def.width,
                        def.active,
                        absolute_x,
                        absolute_z,
                        actual,
                        true,
                    );
                }
            }
        }
        offset = packet.smarts()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_pair_blocks_both_sides() {
        let mut collision = CollisionMap::new();
        collision.change_wall(10, 10, 0, ANGLE_WEST, WALL_STRAIGHT, false, true);
        assert!(collision.is_flagged(10, 10, 0, WALL_WEST));
        assert!(collision.is_flagged(9, 10, 0, WALL_EAST));
        assert!(!step_open(&collision, 10, 10, 0, -1, 0));
        assert!(step_open(&collision, 10, 10, 0, 1, 0));
        collision.change_wall(10, 10, 0, ANGLE_WEST, WALL_STRAIGHT, false, false);
        assert!(step_open(&collision, 10, 10, 0, -1, 0));
    }

    #[test]
    fn unknown_tiles_read_null_and_count_as_blocked() {
        // Missing zones read NULL (0x7FFFFFFF), which has
        // every flag bit set, so unknown tiles are not walkable.
        let collision = CollisionMap::new();
        assert_eq!(collision.get(100, 100, 0), NULL);
        assert!(!collision.has(100, 100, 0));
        assert!(!tile_walkable(NULL));
    }

    #[test]
    fn loc_footprint_masks_ground_layer() {
        let mut collision = CollisionMap::new();
        change_loc_collision(&mut collision, 10, ANGLE_NORTH, false, 2, 3, 0, 5, 5, 0, true);
        assert!(collision.is_flagged(6, 5, 0, LOC));
        assert!(collision.is_flagged(5, 6, 0, LOC));
        assert!(collision.has(6, 5, 0));
        assert!(!collision.has(8, 5, 0));
    }

    #[test]
    fn server_zip_reads_stored_squares() {
        let pack = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("lostcity")
            .join("engine")
            .join("data")
            .join("pack");
        let zipped = pack.join(".cache").join("maps-server.zip");
        if !zipped.is_file() {
            return;
        }
        let squares = map_files(&pack).expect("server maps load");
        assert!(!squares.is_empty(), "at least one map square");
        let lumbridge = squares.iter().find(|(base_x, base_z, _, _)| *base_x == 3200 && *base_z == 3200);
        assert!(lumbridge.is_some(), "Lumbridge square ships in the server zip");
    }
}
