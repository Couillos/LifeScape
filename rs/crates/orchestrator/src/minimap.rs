#![forbid(unsafe_code)]
//! The client's own minimap image: ground, walls and mapscene sprites.
//!
//! World.render2DGround paints the ground, then drawDetail paints the
//! wall bars, diagonal walls and mapscene sprites of every loc, in the
//! same order as the game. This replays both passes on the packed data.
//! The ground squares themselves live here (not in MapCache) to keep
//! each file under 1000 lines; only this renderer reads them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::mapcache::{MapCache, pack_dir};
use crate::png;

const MAP_SIZE: usize = 64;
const LEVELS: usize = 4;

const ACTIVE_RGB: u32 = 0xEE00_00;
const INACTIVE_RGB: u32 = 0xEEEE_EE;

const LAYER_WALL: u8 = 0;
const LAYER_WALL_DECOR: u8 = 1;
const LAYER_GROUND: u8 = 2;
const LAYER_GROUND_DECOR: u8 = 3;

const WALL_STRAIGHT: u8 = 0;
const WALL_L: u8 = 2;
const WALL_SQUARE_CORNER: u8 = 3;
const WALL_DIAGONAL: u8 = 9;

const MAP_FLAG_VIS_BELOW: u32 = 0x08;
const MAP_FLAG_LINK_BELOW: u32 = 0x02;

/// Overlay mask per shape, as the client paints one tile as 4x4 pixels.
const MINIMAP_SHAPE: [[u8; 16]; 13] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1],
    [1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0],
    [0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1],
    [0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1],
    [1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0],
    [1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 0, 1, 1],
    [1, 1, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 1],
];

const MINIMAP_ROTATE: [[usize; 16]; 4] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [12, 8, 4, 0, 13, 9, 5, 1, 14, 10, 6, 2, 15, 11, 7, 3],
    [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
    [3, 7, 11, 15, 2, 6, 10, 14, 1, 5, 9, 13, 0, 4, 8, 12],
];

const WEST_COLUMN: [(usize, usize); 4] = [(0, 0), (0, 1), (0, 2), (0, 3)];
const NORTH_ROW: [(usize, usize); 4] = [(0, 0), (1, 0), (2, 0), (3, 0)];
const EAST_COLUMN: [(usize, usize); 4] = [(3, 0), (3, 1), (3, 2), (3, 3)];
const SOUTH_ROW: [(usize, usize); 4] = [(0, 3), (1, 3), (2, 3), (3, 3)];

fn layer_of(shape: u8) -> u8 {
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

// -- stored zip ------------------------------------------------------------

/// Map-squares zip on disk: the server pack ships it uncompressed, so no
/// inflate is needed. Shared by the live renderer and the static builder.
pub(crate) fn pack_zip() -> PathBuf {
    pack_dir().join(".cache").join("maps-server.zip")
}

/// Entries of a stored-only zip: name to (data offset, size). The server
/// pack ships its map zips uncompressed, so no inflate is needed.
fn zip_entries(archive: &[u8]) -> HashMap<String, (usize, usize)> {
    let mut entries = HashMap::new();
    let mut position = 0usize;
    while position + 30 <= archive.len() {
        if u32::from_le_bytes(archive[position..position + 4].try_into().unwrap_or([0; 4])) != 0x0403_4B50 {
            break;
        }
        let method = u16::from_le_bytes([archive[position + 8], archive[position + 9]]);
        let size = u32::from_le_bytes(archive[position + 18..position + 22].try_into().unwrap_or([0; 4])) as usize;
        let name_len = u16::from_le_bytes([archive[position + 26], archive[position + 27]]) as usize;
        let extra_len = u16::from_le_bytes([archive[position + 28], archive[position + 29]]) as usize;
        if position + 30 + name_len + extra_len > archive.len() {
            break;
        }
        let name = String::from_utf8_lossy(&archive[position + 30..position + 30 + name_len]).into_owned();
        let data = position + 30 + name_len + extra_len;
        if data + size > archive.len() {
            break;
        }
        if method == 0 {
            entries.insert(name, (data, size));
        }
        position = data + size;
    }
    entries
}

// -- ground ------------------------------------------------------------------

#[derive(Clone, Copy)]
struct GroundTile {
    underlay: i32,
    overlay: i32,
    shape: i32,
    rotation: i32,
    flags: u32,
}

impl GroundTile {
    fn missing() -> GroundTile {
        GroundTile { underlay: -1, overlay: -1, shape: -1, rotation: 0, flags: 0 }
    }
}

/// One map square per level: underlay/overlay floor ids plus map flags.
fn parse_square(data: &[u8]) -> Option<Vec<GroundTile>> {
    let mut tiles = vec![GroundTile::missing(); LEVELS * MAP_SIZE * MAP_SIZE];
    let mut position = 0usize;
    for level in 0..LEVELS {
        for tile_x in 0..MAP_SIZE {
            for tile_z in 0..MAP_SIZE {
                let (mut underlay, mut overlay, mut shape, mut rotation, mut flags) = (-1, -1, -1, 0, 0);
                loop {
                    let opcode = *data.get(position)?;
                    position += 1;
                    if opcode == 0 {
                        break;
                    }
                    if opcode == 1 {
                        position += 1;
                        break;
                    }
                    if opcode <= 49 {
                        overlay = i32::from(*data.get(position)?) - 1;
                        position += 1;
                        shape = i32::from((opcode - 2) / 4);
                        rotation = i32::from((opcode - 2) & 3);
                    } else if opcode <= 81 {
                        flags = u32::from(opcode - 49);
                    } else {
                        underlay = i32::from(opcode) - 82;
                    }
                }
                tiles[level * MAP_SIZE * MAP_SIZE + tile_x * MAP_SIZE + tile_z] =
                    GroundTile { underlay, overlay, shape, rotation, flags };
            }
        }
    }
    Some(tiles)
}

/// Map squares live here under a bound: a woodcut loop visits dozens of
/// squares in a few hours, past the old 32-slot random eviction the
/// working set thrashed (every tile re-parsed its square, 81 s a frame
/// with 4 concurrent renders) and one 1.2 ms parse under the lock held
/// every tile lookup behind it.
const SQUARE_CAP: usize = 128;

struct SquareEntry {
    tiles: Arc<Vec<GroundTile>>,
    used: u64,
}

struct Grounds {
    archive: Vec<u8>,
    entries: HashMap<String, (usize, usize)>,
    squares: Mutex<HashMap<(i32, i32), SquareEntry>>,
    clock: std::sync::atomic::AtomicU64,
    blends: Mutex<HashMap<(i32, i32, i32), u32>>,
}

impl Grounds {
    fn load() -> Grounds {
        let archive = std::fs::read(pack_zip()).unwrap_or_default();
        let entries = zip_entries(&archive);
        Grounds { archive, entries, squares: Mutex::new(HashMap::new()), clock: std::sync::atomic::AtomicU64::new(0), blends: Mutex::new(HashMap::new()) }
    }

    /// One map square per level, shared by every tile lookup: the Vec is
    /// cloned once per square (Arc), never once per tile (17k lookups x
    /// 384 KiB per 33-px frame before this). The parse runs outside the
    /// lock and a miss caches nothing: under contention every tile lookup
    /// behind one 1.2 ms critical section stalled the whole server.
    fn square(&self, square_x: i32, square_z: i32) -> Option<Arc<Vec<GroundTile>>> {
        let tick = self.clock.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(cached) = self.squares.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get_mut(&(square_x, square_z)) {
            cached.used = tick;
            return Some(cached.tiles.clone());
        }
        let name = format!("m{square_x}_{square_z}");
        let parsed = self.entries.get(&name).and_then(|(offset, size)| parse_square(&self.archive[*offset..*offset + *size])).map(Arc::new);
        // Off-map squares (None) cache nothing: they used to fill the
        // slots and evict live squares into a re-parse cascade.
        let tiles = parsed?;
        let mut squares = self.squares.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = squares.get_mut(&(square_x, square_z)) {
            cached.used = tick;
            return Some(cached.tiles.clone());
        }
        squares.insert((square_x, square_z), SquareEntry { tiles: tiles.clone(), used: tick });
        // Least-recently-used eviction, one scan past the cap: random
        // eviction could drop a square another render was mid-frame on.
        while squares.len() > SQUARE_CAP {
            if let Some(oldest) = squares.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key) {
                squares.remove(&oldest);
            } else {
                break;
            }
        }
        Some(tiles)
    }

    fn tile(&self, level: i32, tile_x: i32, tile_z: i32) -> GroundTile {
        if !(0..LEVELS as i32).contains(&level) {
            return GroundTile::missing();
        }
        match self.square(tile_x >> 6, tile_z >> 6) {
            Some(tiles) => tiles[level as usize * MAP_SIZE * MAP_SIZE + ((tile_x & 63) as usize) * MAP_SIZE + ((tile_z & 63) as usize)],
            None => GroundTile::missing(),
        }
    }

    /// Display level to packed level: the client shifts every bridged
    /// column down one (`ClientBuild.pushDown`, when `LinkBelow` is set on
    /// level 1), so level L shows packed L+1 and level 3 shows nothing.
    /// Flag checks (`VisBelow`, `ForceHighDetail`) keep reading the raw
    /// packed flags; only the painted content follows the shift.
    fn content_level(&self, level: i32, tile_x: i32, tile_z: i32) -> i32 {
        if (0..LEVELS as i32).contains(&level)
            && self.tile(1, tile_x, tile_z).flags & MAP_FLAG_LINK_BELOW != 0
        {
            level + 1
        } else {
            level
        }
    }

    #[cfg(test)]
    fn content_tile(&self, level: i32, tile_x: i32, tile_z: i32) -> GroundTile {
        self.tile(self.content_level(level, tile_x, tile_z), tile_x, tile_z)
    }

    /// ClientBuild blend over a 10x10 window, x-4..x+5 and z-4..z+5.
    fn blend(&self, cache: &MapCache, level: i32, tile_x: i32, tile_z: i32) -> u32 {
        let blends = self.blends.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = blends.get(&(level, tile_x, tile_z)) {
            return *cached;
        }
        drop(blends);
        let (mut hue_total, mut sat_total, mut lig_total, mut chroma_total, mut count) = (0u64, 0u64, 0u64, 0u64, 0u64);
        for delta_z in -4..6 {
            for delta_x in -4..6 {
                let underlay = self.tile(level, tile_x + delta_x, tile_z + delta_z).underlay;
                if underlay < 0 {
                    continue;
                }
                // The running hue is the chroma-weighted fifth field, not
                // the plain hue first field.
                let (_, saturation, lightness, chroma, weighted) = cache.flo_hsl(underlay);
                hue_total += u64::from(weighted);
                sat_total += u64::from(saturation);
                lig_total += u64::from(lightness);
                chroma_total += u64::from(chroma);
                count += 1;
            }
        }
        let colour = if count == 0 || chroma_total == 0 {
            0
        } else {
            let hue = ((hue_total * 256 / chroma_total) & 0xFF) as u32;
            MapCache::blended_colour(hue, (sat_total / count) as u32, (lig_total / count) as u32)
        };
        let mut blends = self.blends.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Evict half, never clear: a full clear left every in-flight
        // render cold at once and their re-computes piled up.
        if blends.len() > 20000 {
            let drop: Vec<(i32, i32, i32)> = blends.keys().take(10000).copied().collect();
            for key in drop {
                blends.remove(&key);
            }
        }
        blends.insert((level, tile_x, tile_z), colour);
        colour
    }

    fn quick_colour(&self, cache: &MapCache, level: i32, tile_x: i32, tile_z: i32) -> Option<u32> {
        let tile = self.tile(level, tile_x, tile_z);
        if tile.overlay < 0 {
            return Some(if tile.underlay >= 0 { self.blend(cache, level, tile_x, tile_z) } else { 0 });
        }
        if tile.shape <= 0 {
            return Some(cache.flo_overlay_colour(tile.overlay));
        }
        None
    }

    fn paint_tile(&self, cache: &MapCache, pixels: &mut [u32], stride: usize, level: i32, tile_x: i32, tile_z: i32, x: usize, y: usize) {
        let tile = self.tile(level, tile_x, tile_z);
        if tile.overlay < 0 || tile.shape <= 0 {
            if let Some(colour) = self.quick_colour(cache, level, tile_x, tile_z) {
                if colour != 0 {
                    for row in 0..4 {
                        let base = (y + row) * stride + x;
                        for col in 0..4 {
                            pixels[base + col] = colour;
                        }
                    }
                }
            }
            return;
        }
        let blend = if tile.underlay >= 0 { self.blend(cache, level, tile_x, tile_z) } else { 0 };
        let colour = cache.flo_overlay_colour(tile.overlay);
        let cells = MINIMAP_SHAPE[(tile.shape + 1) as usize];
        let turned = MINIMAP_ROTATE[(tile.rotation & 3) as usize];
        for row in 0..4 {
            let base = (y + row) * stride + x;
            for col in 0..4 {
                if cells[turned[row * 4 + col]] != 0 {
                    pixels[base + col] = colour;
                } else if blend != 0 {
                    pixels[base + col] = blend;
                }
            }
        }
    }

    /// Client minimap ground pass for a w x h window; row 0 is the
    /// northern edge. Flag checks read the packed flags, painted tiles
    /// follow the bridge shift (`content_tile`).
    fn render_ground(&self, cache: &MapCache, level: i32, world_x0: i32, world_z0: i32, w: usize, h: usize) -> Vec<u32> {
        let border = w * 4;
        let mut pixels = vec![0u32; border * h * 4];
        for row in 0..h {
            let world_z = world_z0 + row as i32;
            let y = (h - 1 - row) * 4;
            for col in 0..w {
                let world_x = world_x0 + col as i32;
                let x = col * 4;
                if self.tile(level, world_x, world_z).flags & 0x18 == 0 {
                    let content = self.content_level(level, world_x, world_z);
                    self.paint_tile(cache, &mut pixels, border, content, world_x, world_z, x, y);
                }
                if level < LEVELS as i32 - 1 && self.tile(level + 1, world_x, world_z).flags & MAP_FLAG_VIS_BELOW != 0 {
                    let content = self.content_level(level + 1, world_x, world_z);
                    self.paint_tile(cache, &mut pixels, border, content, world_x, world_z, x, y);
                }
            }
        }
        pixels
    }
}

// -- loc layers --------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.position)?;
        self.position += 1;
        Some(byte)
    }

    fn big_u16(&mut self) -> Option<u16> {
        let value = u16::from_be_bytes([*self.data.get(self.position)?, *self.data.get(self.position + 1)?]);
        self.position += 2;
        Some(value)
    }

    /// Smart int: one byte below 0x80, else two bytes minus 0x8000.
    fn smarts(&mut self) -> Option<i32> {
        if *self.data.get(self.position)? < 0x80 {
            Some(i32::from(self.byte()?))
        } else {
            Some(i32::from(self.big_u16()?) - 0x8000)
        }
    }
}

/// (x, z, level, layer) to (loc id, shape, angle), as the client places them.
fn load_loc_layers(archive: &[u8], entries: &HashMap<String, (usize, usize)>) -> Option<HashMap<(i32, i32, i32, u8), (u32, u8, u8)>> {
    let mut squares: Vec<(i32, i32)> = entries
        .keys()
        .filter(|name| name.starts_with('m'))
        .filter_map(|name| {
            let (x, z) = name[1..].split_once('_')?;
            Some((x.parse().ok()?, z.parse().ok()?))
        })
        .collect();
    squares.sort();
    let mut layers = HashMap::new();
    for (square_x, square_z) in squares {
        let ground_name = format!("m{square_x}_{square_z}");
        let locs_name = format!("l{square_x}_{square_z}");
        let (ground_off, ground_len) = entries.get(&ground_name)?;
        let (locs_off, locs_len) = entries.get(&locs_name)?;
        let grounds = &archive[*ground_off..*ground_off + *ground_len];
        // Bridge shift needs the packed flags, like the client's pushDown:
        // a column whose level-1 flags carry LinkBelow shows packed L+1
        // at level L. Unparsable grounds keep the old unshifted placement.
        let flags = parse_square(grounds).unwrap_or_default();
        let locs = &archive[*locs_off..*locs_off + *locs_len];
        let mut packet = Cursor { data: locs, position: 0 };
        let mut loc_id: i32 = -1;
        let mut delta = packet.smarts()?;
        while delta != 0 {
            loc_id += delta;
            let mut coord: i32 = 0;
            let mut coord_offset = packet.smarts()?;
            while coord_offset != 0 {
                coord += coord_offset - 1;
                let tile_z = coord & 0x3F;
                let tile_x = (coord >> 6) & 0x3F;
                let level = (coord >> 12) & 0x3;
                let info = packet.byte()?;
                coord_offset = packet.smarts()?;
                let shape = info >> 2;
                let angle = info & 0x3;
                let bridged = flags
                    .get(MAP_SIZE * MAP_SIZE + tile_x as usize * MAP_SIZE + tile_z as usize)
                    .is_some_and(|tile| tile.flags & MAP_FLAG_LINK_BELOW != 0);
                let actual = if bridged { level - 1 } else { level };
                if actual < 0 {
                    continue;
                }
                layers.insert(
                    (tile_x + (square_x << 6), tile_z + (square_z << 6), actual, layer_of(shape)),
                    (loc_id as u32, shape, angle),
                );
            }
            delta = packet.smarts()?;
        }
    }
    Some(layers)
}

// -- minimap -------------------------------------------------------------------

/// Last PNG frames by (level, x, z, size): a browser refetching the same
/// image after a timeout must not queue another full render behind the
/// herd. Bounded, one slot evicted per overflow, no eviction cliff.
const FRAME_CAP: usize = 16;

pub struct Minimap {
    cache: MapCache,
    grounds: Grounds,
    layers: Mutex<Option<HashMap<(i32, i32, i32, u8), (u32, u8, u8)>>>,
    frames: Mutex<HashMap<(i32, i32, i32, usize), Vec<u8>>>,
    /// One render at a time: concurrent frames serialise here instead of
    /// thrashing the square/blend caches against each other (measured
    /// 81 s for 4 concurrent cold frames, 15 ms serialised). Throughput
    /// is ~60 warm frames/s, the dashboard needs < 10.
    render_lock: Mutex<()>,
}

impl Minimap {
    pub fn new(pack_dir: PathBuf) -> Minimap {
        Minimap {
            cache: MapCache::new(pack_dir.clone()),
            grounds: Grounds::load(),
            layers: Mutex::new(None),
            frames: Mutex::new(HashMap::new()),
            render_lock: Mutex::new(()),
        }
    }

    pub fn from_pack_dir() -> Minimap {
        Minimap::new(pack_dir())
    }

    pub fn cache(&self) -> &MapCache {
        &self.cache
    }

    fn loc_layers(&self) -> std::sync::MutexGuard<'_, Option<HashMap<(i32, i32, i32, u8), (u32, u8, u8)>>> {
        let mut layers = self.layers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if layers.is_none() {
            *layers = load_loc_layers(&self.grounds.archive, &self.grounds.entries);
        }
        layers
    }

    /// RGB pixels for a w x h window, row 0 at world_z0. Details draw from
    /// a padded window so sprites from nearby tiles still land inside.
    fn render(&self, level: i32, world_x0: i32, world_z0: i32, w: usize, h: usize, margin: usize) -> Vec<u32> {
        let left = world_x0 - margin as i32;
        let top = world_z0 - margin as i32;
        let (span_w, span_h) = (w + margin * 2, h + margin * 2);
        let mut pixels = self.grounds.render_ground(&self.cache, level, left, top, span_w, span_h);
        self.draw_details(&mut pixels, level, left, top, span_w, span_h);
        if margin == 0 {
            pixels
        } else {
            crop(&pixels, span_w * 4, margin * 4, w * 4, h * 4)
        }
    }

    /// Whole static-map file content: same passes as one live frame, kept
    /// verbatim so crops stay pixel-identical to `/minimap.png` used to be.
    pub(crate) fn render_static(&self, level: i32, world_x0: i32, world_z0: i32, w: usize, h: usize) -> Vec<u32> {
        self.render(level, world_x0, world_z0, w, h, 10)
    }

    /// Packed squares the zip ships, for the static builder's extents.
    pub(crate) fn map_squares(&self) -> Vec<(i32, i32)> {
        let mut squares: Vec<(i32, i32)> = self
            .grounds
            .entries
            .keys()
            .filter(|name| name.starts_with('m'))
            .filter_map(|name| {
                let (x, z) = name[1..].split_once('_')?;
                Some((x.parse().ok()?, z.parse().ok()?))
            })
            .collect();
        squares.sort();
        squares
    }

    /// Raw zip bytes backing this renderer, for the stamp hash.
    #[cfg(test)]
    pub(crate) fn pack_bytes(&self) -> &[u8] {
        &self.grounds.archive
    }

    fn draw_details(&self, pixels: &mut [u32], level: i32, world_x0: i32, world_z0: i32, w: usize, h: usize) {
        let stride = w * 4;
        let layers = self.loc_layers();
        let Some(layers) = layers.as_ref() else { return };
        for world_z in world_z0..world_z0 + h as i32 {
            for world_x in world_x0..world_x0 + w as i32 {
                if self.grounds.tile(level, world_x, world_z).flags & 0x18 == 0 {
                    self.draw_detail(pixels, stride, level, layers, world_x, world_z, world_x0, world_z0, w, h);
                }
                if level < LEVELS as i32 - 1 && self.grounds.tile(level + 1, world_x, world_z).flags & MAP_FLAG_VIS_BELOW != 0 {
                    self.draw_detail(pixels, stride, level + 1, layers, world_x, world_z, world_x0, world_z0, w, h);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_detail(
        &self,
        pixels: &mut [u32],
        stride: usize,
        level: i32,
        layers: &HashMap<(i32, i32, i32, u8), (u32, u8, u8)>,
        world_x: i32,
        world_z: i32,
        world_x0: i32,
        world_z0: i32,
        w: usize,
        h: usize,
    ) {
        let tile_x = ((world_x - world_x0) * 4) as usize;
        let tile_y = ((h as i32 - 1 - (world_z - world_z0)) * 4) as usize;
        if let Some((loc_id, shape, angle)) = layers.get(&(world_x, world_z, level, LAYER_WALL)) {
            if let Some(loc) = self.cache.loc_entry(*loc_id as i32) {
                if loc.mapscene >= 0 {
                    self.plot_sprite(pixels, world_x, world_z, &loc, world_x0, world_z0, w, h);
                } else {
                    draw_wall(pixels, stride, tile_x, tile_y, *shape, *angle, if loc.active { ACTIVE_RGB } else { INACTIVE_RGB });
                }
            }
        }
        if let Some((loc_id, shape, angle)) = layers.get(&(world_x, world_z, level, LAYER_GROUND)) {
            if let Some(loc) = self.cache.loc_entry(*loc_id as i32) {
                if loc.mapscene >= 0 {
                    self.plot_sprite(pixels, world_x, world_z, &loc, world_x0, world_z0, w, h);
                } else if *shape == WALL_DIAGONAL {
                    let rgb = if loc.active { ACTIVE_RGB } else { INACTIVE_RGB };
                    let diagonal = if *angle == 0 || *angle == 2 {
                        [(0, 3), (1, 2), (2, 1), (3, 0)]
                    } else {
                        [(0, 0), (1, 1), (2, 2), (3, 3)]
                    };
                    for (dx, dy) in diagonal {
                        pixels[(tile_y + dy) * stride + tile_x + dx] = rgb;
                    }
                }
            }
        }
        if let Some((loc_id, _, _)) = layers.get(&(world_x, world_z, level, LAYER_GROUND_DECOR)) {
            if let Some(loc) = self.cache.loc_entry(*loc_id as i32) {
                if loc.mapscene >= 0 {
                    self.plot_sprite(pixels, world_x, world_z, &loc, world_x0, world_z0, w, h);
                }
            }
        }
    }

    fn plot_sprite(
        &self,
        pixels: &mut [u32],
        world_x: i32,
        world_z: i32,
        loc: &crate::mapcache::LocEntry,
        world_x0: i32,
        world_z0: i32,
        w: usize,
        h: usize,
    ) {
        let sprite = match self.cache.mapscene_sprite(loc.mapscene) {
            Some(sprite) => sprite,
            None => return,
        };
        let offset_x = (loc.width * 4 - sprite.width as i32) / 2;
        let offset_y = (loc.length * 4 - sprite.height as i32) / 2;
        let left = (world_x - world_x0) * 4 + offset_x + sprite.xof;
        let top = (h as i32 - 1 - (world_z - world_z0)) * 4 + (1 - loc.length) * 4 + offset_y + sprite.yof;
        let width_px = w as i32 * 4;
        let border = h as i32 * 4;
        let (width, height) = (sprite.width as usize, sprite.height as usize);
        for y in 0..height {
            let dest_y = top + y as i32;
            if dest_y < 0 || dest_y >= border {
                continue;
            }
            for x in 0..width {
                let dest_x = left + x as i32;
                if dest_x < 0 || dest_x >= width_px {
                    continue;
                }
                let index = sprite.pixels[y * width + x];
                if index == 0 || usize::from(index) >= sprite.palette.len() {
                    continue;
                }
                pixels[dest_y as usize * width_px as usize + dest_x as usize] = sprite.palette[usize::from(index)];
            }
        }
    }

    /// PNG for a size x size window at (level, x, z), 4 px per tile.
    /// Identical concurrent requests share one cached frame instead of
    /// queueing N renders behind the same herd.
    pub fn png(&self, level: i32, world_x: i32, world_z: i32, size: usize) -> Vec<u8> {
        let key = (level, world_x, world_z, size);
        // Single-flight: outer render lock first (leaf locks only
        // inside), so identical concurrent requests share the one frame
        // instead of rendering N times.
        let _render = self.render_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(frame) = self.frames.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
            return frame.clone();
        }
        let pixels = self.render(level, world_x, world_z, size, size, 10);
        let mut rgb = Vec::with_capacity(pixels.len() * 3);
        for pixel in pixels {
            rgb.extend_from_slice(&[((pixel >> 16) & 0xFF) as u8, ((pixel >> 8) & 0xFF) as u8, (pixel & 0xFF) as u8]);
        }
        let border = (size * 4) as u32;
        let frame = png::encode_rgb(border, border, &rgb);
        let mut frames = self.frames.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if frames.len() >= FRAME_CAP {
            if let Some(oldest) = frames.keys().next().copied() {
                frames.remove(&oldest);
            }
        }
        frames.insert(key, frame.clone());
        frame
    }
}

fn draw_wall(pixels: &mut [u32], stride: usize, tile_x: usize, tile_y: usize, shape: u8, angle: u8, rgb: u32) {
    if shape == WALL_STRAIGHT || shape == WALL_L {
        for (dx, dy) in [WEST_COLUMN, NORTH_ROW, EAST_COLUMN, SOUTH_ROW][usize::from(angle & 3)] {
            pixels[(tile_y + dy) * stride + tile_x + dx] = rgb;
        }
    }
    if shape == WALL_L {
        for (dx, dy) in [NORTH_ROW, EAST_COLUMN, SOUTH_ROW, WEST_COLUMN][usize::from(angle & 3)] {
            pixels[(tile_y + dy) * stride + tile_x + dx] = rgb;
        }
    }
    if shape == WALL_SQUARE_CORNER {
        let (dx, dy) = [(0, 0), (3, 0), (3, 3), (0, 3)][usize::from(angle & 3)];
        pixels[(tile_y + dy) * stride + tile_x + dx] = rgb;
    }
}

fn crop(pixels: &[u32], stride: usize, margin: usize, w: usize, h: usize) -> Vec<u32> {
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        let start = (y + margin) * stride + margin;
        out[y * w..(y + 1) * w].copy_from_slice(&pixels[start..start + w]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv(pixels: &[u32]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for pixel in pixels {
            hash ^= u64::from(pixel & 0xFF_FFFF);
            hash = hash.wrapping_mul(0x100000001B3);
        }
        hash
    }

    #[test]
    fn concurrent_frames_stay_fast() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        // Non-regression (front mort 2026-11-07: vague 4 x 81 s, puis
        // timeouts): concurrent frames must not re-parse squares in a
        // cascade. Warm up first (one-off whole-world loc index ~1 s),
        // then identical x 10 share one cached frame and distinct spots
        // stay in the millisecond range.
        let maps = Arc::new(Minimap::new(pack_dir()));
        let _ = maps.png(0, 3200, 3200, 9);
        let spots = [(0, 3180, 3200), (0, 3000, 3050), (0, 2900, 2900), (0, 3300, 3100)];
        let mut handles = Vec::new();
        for _ in 0..10 {
            let maps = maps.clone();
            handles.push(std::thread::spawn(move || {
                let start = Instant::now();
                let frame = maps.png(0, 3200, 3200, 33);
                assert!(!frame.is_empty());
                start.elapsed()
            }));
        }
        for (index, (level, x, z)) in spots.iter().enumerate() {
            let maps = maps.clone();
            let (level, x, z) = (*level, *x + index as i32 * 7, *z);
            handles.push(std::thread::spawn(move || {
                let start = Instant::now();
                let frame = maps.png(level, x, z, 33);
                assert!(!frame.is_empty());
                start.elapsed()
            }));
        }
        let mut worst = Duration::ZERO;
        for handle in handles {
            worst = worst.max(handle.join().unwrap());
        }
        assert!(worst < Duration::from_secs(2), "concurrent frames took {worst:?}");
    }

    #[test]
    fn bridge_deck_renders_at_ground_level() {
        // Lumbridge bridge (3240..3250, 3225..3226): the pack stores the
        // deck overlay (id 9) at level 1 with LinkBelow set. The client
        // shifts the whole column down (ClientBuild.pushDown), so level 0
        // shows the deck, not the raw level-0 bank. Same shift for locs:
        // wall 3007 sits at raw level 1 and must surface at level 0.
        let maps = Minimap::new(pack_dir());
        assert!(maps.grounds.tile(1, 3244, 3225).flags & MAP_FLAG_LINK_BELOW != 0, "bridge column flagged");
        assert_eq!(maps.grounds.tile(1, 3244, 3225).overlay, 9, "packed deck overlay");
        let deck = maps.grounds.content_tile(0, 3244, 3225);
        assert_eq!(deck.overlay, 9, "level 0 shows the packed level-1 deck");
        assert_ne!(deck.overlay, maps.grounds.tile(0, 3244, 3225).overlay, "shift changes the paint");
        let layers = maps.loc_layers();
        let wall = layers.as_ref().expect("loc layers load").get(&(3240, 3225, 0, LAYER_WALL)).copied();
        drop(layers);
        assert_eq!(wall, Some((3007u32, 0u8, 3u8)), "bridge wall surfaces at level 0");
        // Pixels: deck colour on the open tile, inactive wall bar over it
        // where the shifted wall sits (south edge, angle 3).
        let deck_rgb = maps.cache().flo_overlay_colour(9);
        assert_ne!(deck_rgb, 0, "deck overlay has a colour");
        let pixels = maps.render(0, 3241, 3222, 7, 7, 0);
        let stride = 7 * 4;
        let at = |tile_x: i32, tile_z: i32, dx: usize, dy: usize| {
            pixels[((6 - (tile_z - 3222)) as usize * 4 + dy) * stride + (tile_x - 3241) as usize * 4 + dx]
        };
        assert_eq!(at(3244, 3225, 0, 0), deck_rgb, "deck paints the tile");
        assert_eq!(at(3244, 3225, 1, 3), INACTIVE_RGB, "shifted wall bar lands on the deck");
    }

    #[test]
    fn ground_matches_reference() {
        let maps = Minimap::new(pack_dir());
        let pixels = maps.grounds.render_ground(maps.cache(), 0, 3200, 3200, 9, 9);
        assert_eq!(pixels.len(), 36 * 36);
        assert_eq!(pixels[0], 0x53670F);
        // Full-window checksum pinned by the reference render.
        assert_eq!(fnv(&pixels), 0x5862_55ce_d1a0_5b95);
    }

    #[test]
    fn minimap_png_matches_reference() {
        let maps = Minimap::new(pack_dir());
        let pixels = maps.render(0, 3200, 3200, 9, 9, 10);
        assert_eq!(fnv(&pixels), 0x9b0b_d800_9182_5b36);
        let png = maps.png(0, 3200, 3200, 9);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 36);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 36);
        assert_eq!(png[25], 2); // truecolor
    }

    /// E12 bound: past FRAME_CAP distinct windows the cache evicts one
    /// slot per overflow and never grows with the spectators.
    #[test]
    fn frame_cache_stays_bounded() {
        let maps = Minimap::new(crate::minimap::pack_dir());
        for index in 0..(super::FRAME_CAP + 6) {
            let frame = maps.png(0, 3000 + index as i32 * 11, 3000, 5);
            assert!(!frame.is_empty());
        }
        let len = maps.frames.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len();
        assert!(len <= super::FRAME_CAP, "cache bounded at {len}");
    }
}
