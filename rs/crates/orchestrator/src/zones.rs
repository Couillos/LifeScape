#![forbid(unsafe_code)]
//! Zone naming from the world map labels the server ships, plus the
//! random-event instances the labels never cover.
//!
//! labels.dat lives in
//! engine/data/pack/mapview/worldmap.jag (count p2; per label: pjstr name
//! + p2 x + p2 z + p1 type). Nothing is invented: the nearest shipped label
//! wins, or none. Random events teleport off the labelled map (maze, mime).

use std::sync::OnceLock;

use crate::mapcache::{bz2_decode, pack_dir};

pub struct Label {
    pub name: String,
    pub pos_x: i32,
    pub pos_z: i32,
    pub kind: u8,
}

fn name_hash(name: &str) -> i32 {
    name.bytes().map(|byte| byte.to_ascii_uppercase()).fold(0u32, |value, byte| {
        value.wrapping_mul(61).wrapping_add(u32::from(byte).wrapping_sub(32))
    }) as i32
}

fn big_u16(data: &[u8]) -> u16 {
    u16::from_be_bytes([data[0], data[1]])
}

fn big_u24(data: &[u8]) -> u32 {
    (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2])
}

fn big_i32(data: &[u8]) -> i32 {
    i32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

/// Raw labels.dat bytes from the worldmap jag, or None when unreadable.
fn labels_dat() -> Option<Vec<u8>> {
    let raw = std::fs::read(pack_dir().join("mapview").join("worldmap.jag")).ok()?;
    if raw.len() < 6 {
        return None;
    }
    let unpacked = big_u24(&raw[0..3]);
    let packed = big_u24(&raw[3..6]);
    let mut body: Vec<u8> = if unpacked != packed {
        let mut full = b"BZh1".to_vec();
        // The body skips the 6-byte split header (body[6:]).
        full.extend_from_slice(raw.get(12..)?);
        bz2_decode(&full)?
    } else {
        raw[6..].to_vec()
    };
    if body.len() < 2 {
        return None;
    }
    let count = big_u16(&body[0..2]) as usize;
    if 2 + count * 10 > body.len() {
        return None;
    }
    let wanted = name_hash("labels.dat");
    let mut start = 2 + count * 10;
    for entry in 0..count {
        let base = 2 + entry * 10;
        let hash = big_i32(&body[base..base + 4]);
        let file_unpacked = big_u24(&body[base + 4..base + 7]);
        let file_packed = big_u24(&body[base + 7..base + 10]);
        let end = start.checked_add(file_packed as usize)?;
        if end > body.len() {
            return None;
        }
        if hash == wanted {
            let chunk = body[start..end].to_vec();
            if file_unpacked == file_packed {
                return Some(chunk);
            }
            let mut full = b"BZh1".to_vec();
            full.extend_from_slice(&chunk);
            return bz2_decode(&full);
        }
        start = end;
    }
    // Keep the borrow checker quiet about the unused mut.
    body.clear();
    None
}

fn load_labels() -> Vec<Label> {
    let Some(data) = labels_dat() else { return Vec::new() };
    if data.len() < 2 {
        return Vec::new();
    }
    let count = big_u16(&data[0..2]) as usize;
    let mut labels = Vec::with_capacity(count);
    let mut position = 2usize;
    for _ in 0..count {
        let Some(end) = data[position..].iter().position(|byte| *byte == 10).map(|at| position + at) else {
            break;
        };
        let name = String::from_utf8_lossy(&data[position..end]).replace('/', " ");
        position = end + 1;
        if position + 5 > data.len() {
            break;
        }
        let label_x = i32::from(big_u16(&data[position..position + 2]));
        let label_z = i32::from(big_u16(&data[position + 2..position + 4]));
        let kind = data[position + 4];
        position += 5;
        labels.push(Label { name, pos_x: label_x, pos_z: label_z, kind });
    }
    labels
}

fn cached_labels() -> &'static Vec<Label> {
    static LABELS: OnceLock<Vec<Label>> = OnceLock::new();
    LABELS.get_or_init(load_labels)
}

/// Event name when (pos_x, pos_z) sits in a random-event instance, else None.
/// Same boxes the rescue logic uses.
pub fn event_area(pos_x: i32, pos_z: i32) -> Option<&'static str> {
    if (2880..2944).contains(&pos_x) && (4544..4608).contains(&pos_z) {
        return Some("maze");
    }
    if (1984..2048).contains(&pos_x) && (4736..4800).contains(&pos_z) {
        return Some("mime");
    }
    None
}

fn type_radius(kind: u8) -> f64 {
    match kind {
        0 => 60.0,
        1 => 120.0,
        _ => 600.0,
    }
}

/// (name, distance) of the closest honest label for a tile, or None.
/// Local places (radius 60) and towns (radius 120) compete by distance;
/// regions (radius 600) are fallback only.
pub fn nearest(pos_x: i32, pos_z: i32) -> Option<(String, f64)> {
    let labels = cached_labels();
    let mut place: Option<(String, f64)> = None;
    for label in labels.iter().filter(|label| label.kind != 2) {
        let dist = (((label.pos_x - pos_x) as f64).powi(2) + ((label.pos_z - pos_z) as f64).powi(2)).sqrt();
        if dist <= type_radius(label.kind) && place.as_ref().is_none_or(|(_, best)| dist < *best) {
            place = Some((label.name.clone(), dist));
        }
    }
    if place.is_some() {
        return place;
    }
    let mut region: Option<(String, f64)> = None;
    for label in labels.iter().filter(|label| label.kind == 2) {
        let dist = (((label.pos_x - pos_x) as f64).powi(2) + ((label.pos_z - pos_z) as f64).powi(2)).sqrt();
        if dist <= type_radius(label.kind) && region.as_ref().is_none_or(|(_, best)| dist < *best) {
            region = Some((label.name.clone(), dist));
        }
    }
    region
}

/// (name, distance) for the bot's tile, or None. Random-event instances
/// come first: the world map has no labels there.
pub fn zone_name(pos_x: i32, pos_z: i32) -> Option<(String, Option<f64>)> {
    if let Some(event) = event_area(pos_x, pos_z) {
        return Some((format!("Random event ({event})"), None));
    }
    nearest(pos_x, pos_z).map(|(name, dist)| (name, Some(dist)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_spots_match_labels() {
        // Spot checks against the shipped labels.
        let (name, dist) = nearest(3098, 3266).expect("Market claims t01");
        assert_eq!(name, "Market");
        assert!((dist - 16.12).abs() < 0.05, "dist {dist}");
        let (name, _) = nearest(3104, 3093).expect("Tutorial Island claims bob");
        assert_eq!(name, "Tutorial Island");
        assert!(nearest(3098, 9567).is_none());
    }

    #[test]
    fn event_areas_name_first() {
        let (name, dist) = zone_name(2900, 4560).expect("maze box");
        assert_eq!(name, "Random event (maze)");
        assert!(dist.is_none());
    }
}
