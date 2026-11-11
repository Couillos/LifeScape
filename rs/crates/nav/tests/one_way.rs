//! Une porte one-way ne se traverse que depuis le cote declare :
//! deux poches 3x3 reliees par une seule porte, comme paid_only.
//! Le sens autorise passe (stand + sortie de tuile), le sens interdit
//! echoue en Disconnected. Non-regression du step-off : sans l'exemption
//! `here == door_tile`, meme le sens autorise serait bloque.

use std::collections::{HashMap, HashSet};

use lifescape_nav::collision::LocDef;
use lifescape_nav::plan::{self, NavOptions, NoRouteReason, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::{OneWay, Side, Tables};

const SQUARE: i32 = 50;
const BASE: i32 = SQUARE << 6;
const DOOR_ID: u16 = 10;
const DOOR_LEVEL: i32 = 0;

fn west_tile() -> Tile {
    Tile { x: BASE + 9, z: BASE + 19, level: DOOR_LEVEL }
}

fn east_tile() -> Tile {
    Tile { x: BASE + 13, z: BASE + 19, level: DOOR_LEVEL }
}

fn door_tile() -> Tile {
    Tile { x: BASE + 11, z: BASE + 19, level: DOOR_LEVEL }
}

fn open_tile(x: i32, z: i32) -> bool {
    let (local_x, local_z) = (x - BASE, z - BASE);
    if local_x == 11 && local_z == 19 {
        return true;
    }
    ((8..=10).contains(&local_x) || (12..=14).contains(&local_x)) && (18..=20).contains(&local_z)
}

fn ground_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    for level in 0..4 {
        for local_x in 0..64 {
            for local_z in 0..64 {
                if level == 0 && open_tile(BASE + local_x, BASE + local_z) {
                    out.push(0);
                } else {
                    out.push(50);
                    out.push(0);
                }
            }
        }
    }
    out
}

fn loc_bytes() -> Vec<u8> {
    vec![11, 0x82, 0xD4, 0x00, 0x00, 0x00]
}

fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let local_offset = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&[0u8; 4]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&local_offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let dir_offset = out.len() as u32;
    out.extend_from_slice(&central);
    let dir_size = central.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&dir_size.to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn setup() -> (std::sync::Arc<StaticWorld>, Tables, std::path::PathBuf) {
    let ground = ground_bytes();
    let locs = loc_bytes();
    let zip = stored_zip(&[("m50_50", &ground), ("l50_50", &locs)]);
    let path = std::env::temp_dir().join(format!("lifescape-one-way-{}.zip", std::process::id()));
    std::fs::write(&path, &zip).expect("write synthetic pack");
    let mut defs = vec![None; DOOR_ID as usize + 1];
    defs[DOOR_ID as usize] = Some(LocDef {
        blockwalk: true,
        blockrange: true,
        length: 1,
        width: 1,
        active: 1,
        has_ops: true,
        open_op: Some(1),
        toll: false,
    });
    let world = StaticWorld::open(&path, defs).expect("synthetic world");
    let tables = Tables {
        transitions: Vec::new(),
        verified: HashMap::new(),
        verified_any_level: Vec::new(),
        proven: HashMap::new(),
        requirements: Vec::new(),
        one_way: vec![OneWay {
            loc: DOOR_ID,
            at: door_tile(),
            passable_from: Side::West,
            reason: "test door swings from the west only".to_string(),
        }],
        customs: Vec::new(),
        blocked: Vec::new(),
        sky_floors: HashSet::new(),
    };
    (world, tables, path)
}

#[test]
fn one_way_door_passes_from_declared_side_only() {    let (world, tables, path) = setup();
    let options = NavOptions::default();
    let crossed = plan::plan(&world, &tables, west_tile(), east_tile(), &options)
        .expect("west stand crosses a west-only door");
    assert!(
        crossed.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { door, .. } if door.loc == DOOR_ID)),
        "allowed sense swings the door",
    );
    let refused = plan::plan(&world, &tables, east_tile(), west_tile(), &options)
        .expect_err("east stand never crosses a west-only door");
    assert_eq!(refused.reason, NoRouteReason::Disconnected, "no other passage: {:?}", refused.reason);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn side_holds_matches_server_coord_rule() {
    let door = Tile { x: 3109, z: 3353, level: 0 };
    let south = Tile { x: 3109, z: 3352, level: 0 };
    let level_east = Tile { x: 3110, z: 3353, level: 0 };
    let north = Tile { x: 3109, z: 3354, level: 0 };
    assert!(Side::NotNorth.holds(south, door), "south stand opens");
    assert!(Side::NotNorth.holds(level_east, door), "level stand opens (live entry stand)");
    assert!(!Side::NotNorth.holds(north, door), "north stand refused (live exit stand)");
    assert!(!Side::South.holds(level_east, door), "strict south misses the level stand");
}
