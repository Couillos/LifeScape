#![forbid(unsafe_code)]
//! Ground colours, loc types and mapscene sprites from the pack.
//! The client jags are bzip2
//! compressed (std-only decoder inside); the stored map-squares zip is
//! walked in minimap.rs by the ground renderer that reads it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use crate::png;

/// Engine data layout: `LIFESCAPE_ENGINE_DIR` wins, else the
/// `lostcity/engine` sibling of the project root, then `data/pack` inside it.
pub(crate) fn pack_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("data").join("pack");
        }
    }
    let root = crate::paths::root();
    match root.parent() {
        Some(parent) => parent.join("lostcity").join("engine").join("data").join("pack"),
        None => root,
    }
}

// -- bzip2 ---------------------------------------------------------------

/// Cursor over the bit stream, most significant bit first.
struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn get_bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.bit_pos >> 3)?;
        self.bit_pos += 1;
        Some(u32::from((byte >> (7 - ((self.bit_pos - 1) & 7))) & 1))
    }

    fn get_bits(&mut self, count: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.get_bit()?;
        }
        Some(value)
    }
}

const HUFFMAN_MAX: usize = 23;
const SELECTOR_GROUP_SIZE: i32 = 50;

/// Bzip2 block CRC: CRC-32 with the same polynomial as PNG but fed most
/// significant bit first (init and final xor all ones).
fn bz_crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let mut value = (index as u32) << 24;
        for _ in 0..8 {
            if value & 0x8000_0000 != 0 {
                value = (value << 1) ^ 0x04C1_1DB7;
            } else {
                value <<= 1;
            }
        }
        *slot = value;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc = table[(((crc >> 24) ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc << 8);
    }
    !crc
}

struct HuffmanTables {
    min_len: i32, max_len: i32,
    limit: [i32; HUFFMAN_MAX],
    base: [i32; HUFFMAN_MAX],
    perm: Vec<u32>,
}

/// Canonical decode tables from one group's code lengths.
fn build_tables(lengths: &[u8], alpha_size: usize) -> Option<HuffmanTables> {
    let (mut min_len, mut max_len) = (32i32, 0i32);
    for symbol in 0..alpha_size {
        min_len = min_len.min(i32::from(lengths[symbol]));
        max_len = max_len.max(i32::from(lengths[symbol]));
    }
    if min_len < 1 || max_len > 20 {
        return None;
    }
    let mut perm = vec![0u32; alpha_size];
    let mut slot = 0;
    for length in min_len..=max_len {
        for symbol in 0..alpha_size {
            if i32::from(lengths[symbol]) == length {
                perm[slot] = symbol as u32;
                slot += 1;
            }
        }
    }
    let mut base = [0i32; HUFFMAN_MAX];
    for symbol in 0..alpha_size {
        base[usize::from(lengths[symbol]) + 1] += 1;
    }
    for index in 1..HUFFMAN_MAX {
        base[index] += base[index - 1];
    }
    let mut limit = [0i32; HUFFMAN_MAX];
    let mut code = 0i32;
    for length in min_len..=max_len {
        code += base[(length + 1) as usize] - base[length as usize];
        limit[length as usize] = code - 1;
        code <<= 1;
    }
    for length in min_len + 1..=max_len {
        base[length as usize] = ((limit[(length - 1) as usize] + 1) << 1) - base[length as usize];
    }
    Some(HuffmanTables { min_len, max_len, limit, base, perm })
}

/// Decompress one bzip2 stream (`BZh` + level digit + blocks). Returns
/// `None` on corrupt input, including CRC mismatch.
pub(crate) fn bz2_decode(input: &[u8]) -> Option<Vec<u8>> {
    if input.len() < 4 || &input[..3] != b"BZh" || !(b'1'..=b'9').contains(&input[3]) {
        return None;
    }
    let block_max = (u32::from(input[3] - b'0') * 100_000) as usize;
    let mut reader = BitReader { data: &input[4..], bit_pos: 0 };
    let mut out = Vec::new();
    let mut combined: u32 = 0;
    loop {
        let magic = (u64::from(reader.get_bits(24)?) << 24) | u64::from(reader.get_bits(24)?);
        if magic == 0x1772_4538_5090 {
            return if reader.get_bits(32)? == combined { Some(out) } else { None };
        }
        if magic != 0x3141_5926_5359 {
            return None;
        }
        let stored_crc = reader.get_bits(32)?;
        if reader.get_bit()? != 0 {
            return None; // randomised blocks predate every pack we read
        }
        let orig_ptr = reader.get_bits(24)? as usize;
        let mut in_use = [false; 256];
        let map = reader.get_bits(16)?;
        for group in 0..16 {
            if (map >> (15 - group)) & 1 == 1 {
                let bitmap = reader.get_bits(16)?;
                for bit in 0..16 {
                    in_use[group * 16 + bit as usize] |= (bitmap >> (15 - bit)) & 1 == 1;
                }
            }
        }
        let seq_to_unseq: Vec<u8> =
            in_use.iter().enumerate().filter(|(_, used)| **used).map(|(symbol, _)| symbol as u8).collect();
        let alpha_size = seq_to_unseq.len() + 2;
        if seq_to_unseq.is_empty() || alpha_size > 258 {
            return None;
        }
        let (group_count, selector_count) = (reader.get_bits(3)? as usize, reader.get_bits(15)? as usize);
        if !(2..=6).contains(&group_count) || selector_count == 0 || selector_count > 18002 {
            return None;
        }
        let mut move_to_front = [0u8, 1, 2, 3, 4, 5];
        let mut selectors = Vec::with_capacity(selector_count);
        for _ in 0..selector_count {
            let mut rank = 0usize;
            while reader.get_bit()? == 1 {
                rank += 1;
                if rank >= group_count {
                    return None;
                }
            }
            move_to_front[0..=rank].rotate_right(1);
            selectors.push(move_to_front[0] as usize);
        }
        let mut tables = Vec::with_capacity(group_count);
        for _ in 0..group_count {
            let mut lengths = vec![0u8; alpha_size];
            let mut current = reader.get_bits(5)? as i32;
            for symbol in 0..alpha_size {
                while reader.get_bit()? == 1 {
                    if !(1..=20).contains(&current) {
                        return None;
                    }
                    current += if reader.get_bit()? == 0 { 1 } else { -1 };
                }
                if !(1..=20).contains(&current) {
                    return None;
                }
                lengths[symbol] = current as u8;
            }
            tables.push(build_tables(&lengths, alpha_size)?);
        }
        // Block symbols to bytes: Huffman decode, MTF unshuffle with
        // RUNA/RUNB run lengths, then the EOB marker ends the block.
        let mut block: Vec<u8> = Vec::new();
        let mut mtf_list: [u8; 256] = std::array::from_fn(|index| index as u8);
        let (mut group_no, mut group_pos, mut group) = (-1i32, 0i32, 0usize);
        let (mut run_active, mut run_base, mut run_bits) = (false, 0u32, 0u32);
        loop {
            if group_pos == 0 {
                group_no += 1;
                if group_no as usize >= selector_count {
                    return None;
                }
                group_pos = SELECTOR_GROUP_SIZE;
                group = selectors[group_no as usize];
            }
            group_pos -= 1;
            let table = &tables[group];
            let mut code_len = table.min_len;
            let mut code = reader.get_bits(code_len as u32)? as i32;
            let symbol = loop {
                if code_len > table.max_len {
                    return None;
                }
                if code <= table.limit[code_len as usize] {
                    break *table.perm.get((code - table.base[code_len as usize]) as usize)?;
                }
                code_len += 1;
                code = (code << 1) | reader.get_bit()? as i32;
            };
            // A run of the MTF front symbol: w+1 in binary without its
            // leading 1, RUNA for 0 and RUNB for 1.
            if symbol <= 1 {
                if !run_active {
                    (run_active, run_base, run_bits) = (true, 0, 0);
                } else if run_bits >= 24 {
                    return None;
                }
                run_base += symbol * (1 << run_bits);
                run_bits += 1;
                continue;
            }
            if run_active {
                run_active = false;
                let copies = (((1u32 << run_bits) + run_base - 1) as usize).max(1);
                if block.len() + copies > block_max {
                    return None;
                }
                let front = seq_to_unseq[usize::from(mtf_list[0])];
                block.extend(std::iter::repeat(front).take(copies));
            }
            if symbol as usize == alpha_size - 1 {
                break;
            }
            let index = (symbol - 1) as usize;
            if index >= seq_to_unseq.len() || block.len() + 1 > block_max {
                return None;
            }
            mtf_list[0..=index].rotate_right(1);
            block.push(seq_to_unseq[usize::from(mtf_list[0])]);
        }
        if orig_ptr >= block.len() && !block.is_empty() {
            return None;
        }
        // Inverse Burrows-Wheeler: T links each last-column row to its
        // first-column slot; walking from orig_ptr yields the bytes back
        // to front, so the block fills from the end.
        let mut transformed = vec![0u8; block.len()];
        let mut counts = [0u32; 256];
        for byte in &block {
            counts[usize::from(*byte)] += 1;
        }
        let mut starts = [0u32; 256];
        for value in 1..256 {
            starts[value] = starts[value - 1] + counts[value - 1];
        }
        let mut next = vec![0u32; block.len()];
        for (row, byte) in block.iter().enumerate() {
            next[row] = starts[usize::from(*byte)];
            starts[usize::from(*byte)] += 1;
        }
        let mut row = orig_ptr;
        for slot in transformed.iter_mut().rev() {
            *slot = block[row];
            row = next[row] as usize;
        }
        // The compressor run-length encodes its input first: once four
        // identical bytes are out, the next byte is a repeat count, so
        // invert that here. The block CRC covers these original bytes.
        let mut decoded = Vec::with_capacity(transformed.len());
        let (mut run_byte, mut run_len) = (0u8, 0usize);
        for byte in transformed {
            if run_len == 4 {
                run_len = 0;
                if decoded.len() + byte as usize > block_max {
                    return None;
                }
                decoded.extend(std::iter::repeat(run_byte).take(byte as usize));
            } else {
                decoded.push(byte);
                (run_byte, run_len) = if byte == run_byte && run_len > 0 { (run_byte, run_len + 1) } else { (byte, 1) };
                if decoded.len() > block_max {
                    return None;
                }
            }
        }
        if bz_crc32(&decoded) != stored_crc {
            return None;
        }
        combined = combined.rotate_left(1) ^ stored_crc;
        out.extend_from_slice(&decoded);
    }
}

// -- jag files -----------------------------------------------------------

fn big_u16(data: &[u8]) -> u16 { u16::from_be_bytes([data[0], data[1]]) }

fn big_u24(data: &[u8]) -> u32 { (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2]) }

fn big_i32(data: &[u8]) -> i32 { i32::from_be_bytes([data[0], data[1], data[2], data[3]]) }

fn at(data: &[u8], position: usize) -> Option<u8> { data.get(position).copied() }

/// Jagex archive entry, mirroring orchestration/mapcache `_jagfile_read`.
fn jag_read(container: &[u8], wanted: &str) -> Option<Vec<u8>> {
    if container.len() < 8 {
        return None;
    }
    let unpacked = big_u24(&container[0..3]);
    let packed = big_u24(&container[3..6]);
    let whole = unpacked != packed;
    let data: Vec<u8> = if whole {
        let mut full = b"BZh9".to_vec();
        full.extend_from_slice(&container[6..]);
        bz2_decode(&full)?
    } else {
        container.to_vec()
    };
    if data.len() < 8 {
        return None;
    }
    let count = big_u16(&data[6..8]) as usize;
    if 8 + count * 10 > data.len() {
        return None;
    }
    let mut entries = Vec::with_capacity(count);
    for entry in 0..count {
        let position = 8 + entry * 10;
        entries.push((big_i32(&data[position..position + 4]), big_u24(&data[position + 4..position + 7]), big_u24(&data[position + 7..position + 10])));
    }
    let wanted_hash = name_hash(wanted);
    let mut start = 8 + count * 10;
    for (hash, file_unpacked, file_packed) in entries {
        let end = start.checked_add(file_packed as usize)?;
        if end > data.len() {
            return None;
        }
        if hash == wanted_hash {
            let raw = &data[start..end];
            if whole || file_unpacked == file_packed {
                return Some(raw.to_vec());
            }
            let mut full = b"BZh9".to_vec();
            full.extend_from_slice(raw);
            return bz2_decode(&full);
        }
        start = end;
    }
    None
}

fn name_hash(name: &str) -> i32 {
    name.bytes().map(|byte| byte.to_ascii_uppercase()).fold(0u32, |value, byte| value.wrapping_mul(61).wrapping_add(u32::from(byte).wrapping_sub(32))) as i32
}

// -- client colour chain ---------------------------------------------------

fn gamma_rgb(rgb: u32, factor: f64) -> u32 {
    let mut value = 0u32;
    for shift in [16, 8, 0] {
        let channel = f64::from((rgb >> shift) & 0xFF);
        value |= (((channel / 256.0).powf(factor) * 256.0) as u32 & 0xFF) << shift;
    }
    value
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> u32 {
    let (mut red, mut green, mut blue) = (lightness, lightness, lightness);
    if saturation != 0.0 {
        let second = if lightness < 0.5 { lightness * (saturation + 1.0) } else { lightness + saturation - lightness * saturation };
        let first = lightness * 2.0 - second;
        let mut channels = [0.0; 3];
        for (slot, shift) in channels.iter_mut().zip([1.0 / 3.0, 0.0, -1.0 / 3.0]) {
            let mut position = hue + shift;
            if position > 1.0 {
                position -= 1.0;
            } else if position < 0.0 {
                position += 1.0;
            }
            *slot = if position * 6.0 < 1.0 {
                first + (second - first) * 6.0 * position
            } else if position * 2.0 < 1.0 {
                second
            } else if position * 3.0 < 2.0 {
                first + (second - first) * (2.0 / 3.0 - position) * 6.0
            } else {
                first
            };
        }
        (red, green, blue) = (channels[0], channels[1], channels[2]);
    }
    ((red * 256.0) as u32 & 0xFF) << 16 | ((green * 256.0) as u32 & 0xFF) << 8 | (blue * 256.0) as u32 & 0xFF
}

/// Pix3D colour table entry for the default brightness.
fn colour_table(index: u32) -> u32 {
    let hue = f64::from(((index >> 7) / 8) & 0xFFFF) / 64.0 + 0.007_812_5;
    let saturation = f64::from(((index >> 7) & 0x7) as u8) / 8.0 + 0.062_5;
    let lightness = f64::from((index & 0x7F) as u8) / 128.0;
    gamma_rgb(hsl_to_rgb(hue, saturation, lightness), 0.8)
}

fn hsl_index(hue: u32, mut saturation: u32, lightness: u32) -> u32 {
    for threshold in [179, 192, 217, 243] {
        if lightness > threshold {
            saturation /= 2;
        }
    }
    ((hue / 4) << 10) + ((saturation / 32) << 7) + (lightness / 2)
}

/// Flo rgb to HSL, the way FloType stores it, plus chroma details.
fn floor_colour(rgb: u32) -> (u32, u32, u32, u32, u32) {
    let red = f64::from((rgb >> 16) & 0xFF) / 256.0;
    let green = f64::from((rgb >> 8) & 0xFF) / 256.0;
    let blue = f64::from(rgb & 0xFF) / 256.0;
    let low = red.min(green).min(blue);
    let high = red.max(green).max(blue);
    let lightness = (low + high) / 2.0;
    let (mut hue, mut saturation) = (0.0, 0.0);
    if low != high {
        saturation = (high - low) / if lightness < 0.5 { high + low } else { 2.0 - high - low };
        hue = (if red == high {
            (green - blue) / (high - low)
        } else if green == high {
            (blue - red) / (high - low) + 2.0
        } else {
            (red - green) / (high - low) + 4.0
        }) / 6.0;
    }
    let hue_int = (hue * 256.0) as u32;
    let saturation_int = (saturation * 256.0) as u32;
    let lightness_int = (lightness * 256.0) as u32;
    let chroma = ((if lightness > 0.5 { 1.0 - lightness } else { lightness }) * saturation * 512.0) as u32;
    let chroma = chroma.max(1);
    let underlay_hue = (hue * chroma as f64) as u32;
    (hue_int, saturation_int.min(255), lightness_int.min(255), chroma, underlay_hue)
}

fn scaled_index(index: u32) -> u32 {
    (index & 0xFF80) + (96 * (index & 0x7F) / 128).clamp(2, 126)
}

// -- cache ---------------------------------------------------------------

#[derive(Clone, Copy)]
pub(crate) struct LocEntry {
    pub mapscene: i32,
    pub width: i32,
    pub length: i32,
    pub active: bool,
}

#[derive(Clone)]
struct SpriteMeta {
    width: u32,
    height: u32,
    xof: i32,
    yof: i32,
    encoding: u8,
    data_pos: usize,
    palette: Vec<u32>,
}

/// Decoded mapscene sprite: paletted pixels plus the palette.
#[derive(Clone)]
pub(crate) struct MapSprite {
    pub width: u32,
    pub height: u32,
    pub xof: i32,
    pub yof: i32,
    pub pixels: Vec<u8>,
    pub palette: Vec<u32>,
}

struct FloTables {
    rgb: Vec<u32>,
    texture: Vec<i32>,
    coloured: Vec<bool>,
}

struct Inner {
    flo: Option<FloTables>,
    locs: Option<Vec<LocEntry>>,
    media: Option<Option<Vec<u8>>>,
    textures_jag: Option<Option<Vec<u8>>>,
    sprite_meta: HashMap<i32, Option<SpriteMeta>>,
    sprites: HashMap<i32, Option<MapSprite>>,
    mapscene_png: HashMap<i32, Vec<u8>>,
    texture_avg: HashMap<i32, u32>,
    flo_hsl: HashMap<u32, (u32, u32, u32, u32, u32)>,
}

impl Inner {
    fn new() -> Inner {
        Inner { flo: None, locs: None, media: None, textures_jag: None, sprite_meta: HashMap::new(), sprites: HashMap::new(), mapscene_png: HashMap::new(), texture_avg: HashMap::new(), flo_hsl: HashMap::new() }
    }
}

fn lock_cache(mutex: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ground colours, loc types and mapscene sprites from the pack.
pub struct MapCache {
    pack: PathBuf,
    inner: Mutex<Inner>,
}

impl MapCache {
    pub fn new(pack_dir: PathBuf) -> MapCache {
        MapCache { pack: pack_dir, inner: Mutex::new(Inner::new()) }
    }

    fn config_file(&self) -> Option<Vec<u8>> {
        std::fs::read(self.pack.join("client").join("config")).ok()
    }

    fn load_flo(&self, inner: &mut Inner) {
        if inner.flo.is_some() {
            return;
        }
        let (mut rgb, mut texture, mut coloured) = (Vec::new(), Vec::new(), Vec::new());
        let data = self.config_file().and_then(|container| jag_read(&container, "flo.dat")).or_else(|| {
            // No client jag (dev checkout without a pack): the server dump
            // parses with the same opcodes.
            std::fs::read(self.pack.join("server").join("flo.dat")).ok()
        });
        if let Some(data) = data.filter(|data| data.len() >= 2) {
            let count = big_u16(&data[0..2]) as usize;
            let mut position = 2;
            for _ in 0..count {
                let (mut floor_rgb, mut floor_texture, mut has_colour) = (0u32, -1i32, false);
                while let Some(opcode) = at(&data, position) {
                    position += 1;
                    if opcode == 0 {
                        break;
                    }
                    if opcode == 1 {
                        if position + 3 > data.len() {
                            break;
                        }
                        floor_rgb = big_u24(&data[position..position + 3]);
                        has_colour = true;
                        position += 3;
                    } else if opcode == 2 {
                        floor_texture = at(&data, position).map_or(floor_texture, i32::from);
                        position += 1;
                    } else if opcode == 6 {
                        while at(&data, position).is_some_and(|byte| byte != 10) {
                            position += 1;
                        }
                        position += 1;
                    }
                }
                rgb.push(floor_rgb);
                texture.push(floor_texture);
                coloured.push(has_colour);
            }
        }
        inner.flo = Some(FloTables { rgb, texture, coloured });
    }

    fn flo_tables(&self) -> std::sync::MutexGuard<'_, Inner> {
        let mut inner = lock_cache(&self.inner);
        self.load_flo(&mut inner);
        inner
    }

    /// Flo HSL, chroma and chroma-weighted hue, cached. Textured floors
    /// carry no colour, so every field stays 0 like the client.
    pub(crate) fn flo_hsl(&self, floor: i32) -> (u32, u32, u32, u32, u32) {
        if floor < 0 { return (0, 0, 0, 0, 0); }
        let id = floor as u32;
        let mut inner = self.flo_tables();
        if let Some(cached) = inner.flo_hsl.get(&id) {
            return *cached;
        }
        let hsl = match inner.flo.as_ref() {
            Some(tables) if (id as usize) < tables.coloured.len() && tables.coloured[id as usize] => {
                floor_colour(tables.rgb[id as usize])
            }
            _ => (0, 0, 0, 0, 0),
        };
        inner.flo_hsl.insert(id, hsl);
        hsl
    }

    /// Colour the client paints on the mask cells of an overlay tile:
    /// textured floors take the texture average, magenta paints nothing.
    pub(crate) fn flo_overlay_colour(&self, floor: i32) -> u32 {
        if floor >= 0 {
            let inner = self.flo_tables();
            let known = inner.flo.as_ref().and_then(|tables| {
                tables.rgb.get(floor as usize).map(|rgb| (tables.texture[floor as usize], *rgb))
            });
            drop(inner);
            if let Some((texture, rgb)) = known {
                if texture >= 0 {
                    return self.texture_average(texture);
                }
                if rgb == 0xFF00_FF {
                    return 0;
                }
            }
        }
        let (hue, saturation, lightness, _, _) = self.flo_hsl(floor);
        colour_table(scaled_index(hsl_index(hue, saturation, lightness)))
    }

    /// ClientBuild neighbourhood blend to a table colour (getUCol 96).
    pub(crate) fn blended_colour(hue: u32, saturation: u32, lightness: u32) -> u32 {
        colour_table(scaled_index(hsl_index(hue & 0xFF, saturation, lightness)))
    }

    fn load_locs(&self, inner: &mut Inner) {
        if inner.locs.is_some() {
            return;
        }
        let mut table = Vec::new();
        if let Some(container) = self.config_file() {
            if let (Some(data), Some(index)) = (jag_read(&container, "loc.dat"), jag_read(&container, "loc.idx")) {
                if index.len() >= 2 {
                    let count = big_u16(&index[0..2]) as usize;
                    let mut offset = 2usize;
                    let mut cursor = 2usize;
                    for _ in 0..count {
                        let tile_offset = offset;
                        if cursor + 2 <= index.len() {
                            offset += big_u16(&index[cursor..cursor + 2]) as usize;
                            cursor += 2;
                        }
                        table.push(decode_loc_type(&data, tile_offset));
                    }
                }
            }
        }
        inner.locs = Some(table);
    }

    pub(crate) fn loc_entry(&self, loc_id: i32) -> Option<LocEntry> {
        if loc_id < 0 { return None; }
        let mut inner = lock_cache(&self.inner);
        self.load_locs(&mut inner);
        inner.locs.as_ref()?.get(loc_id as usize).copied()
    }

    /// Mapscene sprite drawn for a loc on the minimap, -1 when unknown.
    pub fn mapscene_for(&self, loc_id: i32) -> i32 {
        self.loc_entry(loc_id).map(|entry| entry.mapscene).unwrap_or(-1)
    }

    fn media_bytes(&self, inner: &mut Inner) -> Option<Vec<u8>> {
        inner.media.get_or_insert_with(|| std::fs::read(self.pack.join("client").join("media")).ok()).clone()
    }

    /// Pix8 header for one mapscene sprite, cached (misses included).
    fn sprite_meta(&self, sprite: i32) -> Option<SpriteMeta> {
        if sprite < 0 { return None; }
        let mut inner = lock_cache(&self.inner);
        self.sprite_meta_locked(&mut inner, sprite)
    }

    fn sprite_meta_locked(&self, inner: &mut Inner, sprite: i32) -> Option<SpriteMeta> {
        if let Some(cached) = inner.sprite_meta.get(&sprite) {
            return cached.clone();
        }
        let media = self.media_bytes(inner);
        let meta = media.as_deref().and_then(|jag| decode_sprite_meta(jag, sprite as u32));
        inner.sprite_meta.insert(sprite, meta.clone());
        meta
    }

    /// Paletted sprite for minimap plotting, cached (misses included).
    pub(crate) fn mapscene_sprite(&self, sprite: i32) -> Option<MapSprite> {
        if sprite < 0 { return None; }
        let mut inner = lock_cache(&self.inner);
        if let Some(cached) = inner.sprites.get(&sprite) {
            return cached.clone();
        }
        let media = self.media_bytes(&mut inner);
        let meta = self.sprite_meta_locked(&mut inner, sprite);
        let decoded = match (media, meta) {
            (Some(jag), Some(meta)) => {
                let container = jag_read(&jag, "mapscene.dat")?;
                let size = meta.width as usize * meta.height as usize;
                if meta.data_pos + size > container.len() {
                    None
                } else {
                    let mut pixels = vec![0u8; size];
                    if meta.encoding == 0 {
                        pixels.copy_from_slice(&container[meta.data_pos..meta.data_pos + size]);
                    } else {
                        let (width, height) = (meta.width as usize, meta.height as usize);
                        for x in 0..width {
                            for y in 0..height {
                                pixels[x + y * width] = container[meta.data_pos + x * height + y];
                            }
                        }
                    }
                    Some(MapSprite { width: meta.width, height: meta.height, xof: meta.xof, yof: meta.yof, pixels, palette: meta.palette })
                }
            }
            _ => None,
        };
        inner.sprites.insert(sprite, decoded.clone());
        decoded
    }

    /// PNG bytes for one mapscene sprite: palette index 0 is transparent.
    pub fn mapscene_png(&self, sprite: i32) -> Option<Vec<u8>> {
        if sprite < 0 { return None; }
        let inner = lock_cache(&self.inner);
        if let Some(cached) = inner.mapscene_png.get(&sprite) {
            return Some(cached.clone());
        }
        drop(inner);
        let sprite_data = self.mapscene_sprite(sprite)?;
        let mut rgba = Vec::with_capacity(sprite_data.pixels.len() * 4);
        for index in sprite_data.pixels {
            match sprite_data.palette.get(usize::from(index)) {
                Some(rgb) if index != 0 => {
                    rgba.extend_from_slice(&[((rgb >> 16) & 0xFF) as u8, ((rgb >> 8) & 0xFF) as u8, (rgb & 0xFF) as u8, 255]);
                }
                _ => rgba.extend_from_slice(&[0, 0, 0, 0]),
            }
        }
        let png = png::encode_rgba(sprite_data.width, sprite_data.height, &rgba);
        lock_cache(&self.inner).mapscene_png.insert(sprite, png.clone());
        Some(png)
    }

    /// Sprite size in pixels, for layout; `None` when unknown.
    pub fn mapscene_size(&self, sprite: i32) -> Option<(u32, u32)> {
        let meta = self.sprite_meta(sprite)?;
        Some((meta.width, meta.height))
    }

    /// Sprite origin offset the image itself carries.
    pub fn mapscene_origin(&self, sprite: i32) -> (i32, i32) {
        self.sprite_meta(sprite).map(|meta| (meta.xof, meta.yof)).unwrap_or((0, 0))
    }

    fn texture_average(&self, texture: i32) -> u32 {
        if texture < 0 { return 1; }
        let mut inner = lock_cache(&self.inner);
        if let Some(cached) = inner.texture_avg.get(&texture) {
            return *cached;
        }
        let jag = inner.textures_jag.get_or_insert_with(|| std::fs::read(self.pack.join("client").join("textures")).ok()).clone();
        let palette = jag.and_then(|jag| {
            let dat = jag_read(&jag, &format!("{texture}.dat"))?;
            decode_texture_palette(&dat, &jag_read(&jag, "index.dat")?)
        });
        let mut totals = [0u64; 3];
        let mut count = 1u64;
        if let Some(palette) = &palette {
            for colour in palette {
                let corrected = gamma_rgb(*colour, 0.8);
                totals[0] += u64::from((corrected >> 16) & 0xFF);
                totals[1] += u64::from((corrected >> 8) & 0xFF);
                totals[2] += u64::from(corrected & 0xFF);
            }
            count = palette.len().max(1) as u64;
        }
        let rgb = ((totals[0] / count) << 16) as u32 | ((totals[1] / count) << 8) as u32 | (totals[2] / count) as u32;
        let average = if palette.is_some() { gamma_rgb(rgb, 1.4).max(1) } else { 1 };
        inner.texture_avg.insert(texture, average);
        average
    }
}

/// Pix8 palette: size byte at `position + 4`, entries after it.
/// Returns the palette and the cursor past the last entry.
fn pix_palette(index: &[u8], position: usize) -> Option<(Vec<u32>, usize)> {
    let mut palette = vec![0u32; usize::from(*index.get(position + 4)?).max(1)];
    let mut cursor = position + 5;
    for slot in 1..palette.len() {
        if cursor + 3 > index.len() {
            return None;
        }
        palette[slot] = big_u24(&index[cursor..cursor + 3]);
        cursor += 3;
    }
    Some((palette, cursor))
}

/// One Pix8 header from a media jag, mirroring `_media_sprite_index`.
fn decode_sprite_meta(media: &[u8], sprite: u32) -> Option<SpriteMeta> {
    let container = jag_read(media, "mapscene.dat")?;
    let index = jag_read(media, "index.dat")?;
    if container.len() < 2 {
        return None;
    }
    let mut position = big_u16(&container[0..2]) as usize;
    if position + 5 > index.len() {
        return None;
    }
    let (palette, after_palette) = pix_palette(&index, position)?;
    position = after_palette;
    let mut data_pos = 2usize;
    for _ in 0..sprite {
        if position + 7 > index.len() {
            return None;
        }
        position += 2;
        data_pos += big_u16(&index[position..position + 2]) as usize * big_u16(&index[position + 2..position + 4]) as usize;
        position += 5;
    }
    if position + 7 > index.len() {
        return None;
    }
    let xof = i32::from(at(&index, position)?);
    let yof = i32::from(at(&index, position + 1)?);
    let width = big_u16(&index[position + 2..position + 4]);
    let height = big_u16(&index[position + 4..position + 6]);
    let encoding = at(&index, position + 6)?;
    if width == 0 || height == 0 || width > 512 || height > 512 {
        return None;
    }
    Some(SpriteMeta { width: u32::from(width), height: u32::from(height), xof, yof, encoding, data_pos, palette })
}

/// Palette of one texture from the textures jag.
fn decode_texture_palette(dat: &[u8], index: &[u8]) -> Option<Vec<u32>> {
    if dat.len() < 2 {
        return None;
    }
    let position = big_u16(&dat[0..2]) as usize;
    if position + 5 > index.len() {
        return None;
    }
    pix_palette(index, position).map(|(palette, _)| palette)
}

/// One loc type: only the fields the spectator needs.
fn decode_loc_type(data: &[u8], mut position: usize) -> LocEntry {
    let (mut mapscene, mut width, mut length) = (-1i32, 1i32, 1i32);
    let mut explicit_active = None;
    let mut has_model = false;
    let mut first_shape = None;
    let mut has_op = false;
    while let Some(opcode) = at(data, position) {
        position += 1;
        match opcode {
            0 => break,
            1 => {
                let count = at(data, position).unwrap_or(0);
                has_model = count > 0;
                first_shape = if count > 0 { at(data, position + 3) } else { None };
                position += 1 + usize::from(count) * 3;
            }
            2 | 3 => {
                position += 1;
                while at(data, position).is_some_and(|byte| byte != 10) {
                    position += 1;
                }
                position += 1;
            }
            5 => {
                let count = at(data, position).unwrap_or(0);
                has_model = count > 0;
                position += 1 + usize::from(count) * 2;
            }
            14 => {
                width = i32::from(at(data, position).unwrap_or(1));
                position += 1;
            }
            15 => {
                length = i32::from(at(data, position).unwrap_or(1));
                position += 1;
            }
            17 | 18 | 21 | 22 | 23 => {}
            19 => {
                explicit_active = Some(at(data, position).unwrap_or(0) == 1);
                position += 1;
            }
            28 | 29 | 39 => position += 1,
            24 => position += 2,
            30..=38 => {
                position += 1;
                let start = position;
                while at(data, position).is_some_and(|byte| byte != 10) {
                    position += 1;
                }
                has_op = has_op || position > start;
                position += 1;
            }
            40 => position += 1 + usize::from(at(data, position).unwrap_or(0)) * 4,
            60 => position += 2,
            62 | 64 => {}
            65 | 66 | 67 | 68 => {
                if opcode == 68 && position + 2 <= data.len() {
                    mapscene = i32::from(big_u16(&data[position..position + 2]));
                }
                position += 2;
            }
            69 | 75 => position += 1,
            70 | 71 | 72 => position += 2,
            _ => {}
        }
    }
    let active = explicit_active.unwrap_or_else(|| (has_model && first_shape.is_none_or(|shape| shape == 10)) || has_op);
    LocEntry { mapscene, width, length, active }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bz2(raw: &[u8]) -> Vec<u8> {
        // The stored stream drops the magic, tests add it back.
        let mut full = b"BZh9".to_vec();
        full.extend_from_slice(raw);
        bz2_decode(&full).expect("valid bzip2 must decode")
    }

    #[test]
    fn bzip2_short_text() {
        // bz2.compress(b'hello world') with the 4 magic bytes stripped.
        let stored = [
            0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x44, 0xF7, 0x13, 0x78, 0x00, 0x00, 0x01, 0x91, 0x80, 0x40,
            0x00, 0x06, 0x44, 0x90, 0x80, 0x20, 0x00, 0x22, 0x03, 0x34, 0x84, 0x30, 0x21, 0xB6, 0x81, 0x54,
            0x27, 0x8B, 0xB9, 0x22, 0x9C, 0x28, 0x48, 0x22, 0x7B, 0x89, 0xBC, 0x00,
        ];
        assert_eq!(bz2(&stored), b"hello world");
    }

    #[test]
    fn bzip2_long_run() {
        // bz2.compress(b'A' * 300): multi-symbol RUNA/RUNB run.
        let stored = [
            0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x0C, 0x6F, 0xB8, 0x15, 0x00, 0x00, 0x02, 0x94, 0x00, 0x80,
            0x20, 0x20, 0x00, 0x00, 0x08, 0x20, 0x00, 0x20, 0xAA, 0x6D, 0x41, 0x98, 0xC5, 0x47, 0x8B, 0xB9,
            0x22, 0x9C, 0x28, 0x48, 0x06, 0x37, 0xDC, 0x0A, 0x80,
        ];
        assert_eq!(bz2(&stored), vec![b'A'; 300]);
    }

    #[test]
    fn bzip2_empty_stream() {
        // bz2.compress(b''): header plus end marker, no block.
        assert_eq!(bz2(&[0x17, 0x72, 0x45, 0x38, 0x50, 0x90, 0x00, 0x00, 0x00, 0x00]), b"");
    }

    #[test]
    fn bzip2_rejects_garbage() {
        assert!(bz2_decode(b"BZh9not a stream").is_none());
        assert!(bz2_decode(b"nope").is_none());
    }

    #[test]
    fn loc_mapscene_vectors() {
        let cache = MapCache::new(pack_dir());
        for (loc, sprite) in [(10, 7), (83, 22), (407, 14), (0, -1), (-1, -1), (999_999, -1)] {
            assert_eq!(cache.mapscene_for(loc), sprite);
        }
    }

    #[test]
    fn mapscene_sprite_meta() {
        let cache = MapCache::new(pack_dir());
        assert_eq!(cache.mapscene_size(0), Some((8, 8)));
        assert_eq!(cache.mapscene_origin(0), (0, 0));
        assert!(cache.mapscene_png(999_999).is_none());
        let png = cache.mapscene_png(7).expect("sprite 7 exists");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let size = (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()));
        assert_eq!(size, (6, 8));
    }

    #[test]
    fn texture_average_vector() {
        let cache = MapCache::new(pack_dir());
        assert_eq!(cache.texture_average(3), 4_399_889);
    }
}
