//! Packed map loading: stored-only zip reader and 64x64 square painter.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::buffer::{ReadError, Reader};
use super::flags::*;
use super::gate_swing::ClosedLeaf;
use super::loc_collision::{CollisionMap, LocDef, change_loc_collision, layer_of};

/// Priority for the single-slot static table: actionable/blocking locs
/// (doors, centre statue) win over stacked ground decor (maze dug soil).
fn score(def: Option<LocDef>) -> u8 {
    match def {
        None => 0,
        Some(entry) => u8::from(entry.blockwalk) * 2 + u8::from(entry.has_ops),
    }
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
    let mut gate_leaves = Vec::new();
    load_locs(
        collision.as_mut().map(|map| &mut **map),
        &lands,
        locs,
        square_x,
        square_z,
        loctypes,
        static_locs,
        static_actions,
        static_layers,
        &mut gate_leaves,
    )?;
    // Open double gates wall the tiles past the fence (see gate_swing).
    if let Some(map) = collision {
        super::gate_swing::paint_gate_swings(map, &gate_leaves);
    }
    Ok(())
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
        static_layers: &mut StaticLayers,
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
        if paint_square(
            Some(collision),
            &ground,
            &locs,
            square_x << 6,
            square_z << 6,
            &self.defs,
            static_locs,
            &mut actions,
            static_layers,
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
        let mut layers = StaticLayers::new();
        for (square_x, square_z) in squares {
            self.ensure_square(collision, static_locs, &mut layers, doors, toll_gates, square_x, square_z);
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
    gate_leaves: &mut Vec<ClosedLeaf>,
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
            if let Some(leaf) = loc.and_then(|def| def.gate_leaf) {
                gate_leaves.push(ClosedLeaf { x: absolute_x, z: absolute_z, level: actual, leaf, shape, angle });
            }
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
