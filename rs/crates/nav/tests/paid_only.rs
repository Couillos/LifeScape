//! La branche PaidOnly ne peut pas apparaitre en or : les deux seuls
//! locs a peage du jeu (2882/2883, deux moities de la porte d'Al-Kharid)
//! sont contournables a pied (detour de 220 tuiles mesure). Ce test force
//! la branche avec un monde synthetique minimal : un carre 64x64 rempli
//! de sol bloque sauf deux poches 3x3 reliees par une seule porte a peage.
//! Sans `paid` le plan echoue en PaidOnly, avec il traverse la porte.

use std::collections::{HashMap, HashSet};

use lifescape_nav::collision::LocDef;
use lifescape_nav::plan::{self, NavOptions, NoRouteReason, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

const SQUARE: i32 = 50;
const BASE: i32 = SQUARE << 6;
const DOOR_ID: u16 = 10;
const DOOR_LEVEL: i32 = 0;

fn from_tile() -> Tile {
    Tile { x: BASE + 9, z: BASE + 19, level: DOOR_LEVEL }
}

fn dest_tile() -> Tile {
    Tile { x: BASE + 13, z: BASE + 19, level: DOOR_LEVEL }
}

fn open_tile(x: i32, z: i32) -> bool {
    let (local_x, local_z) = (x - BASE, z - BASE);
    // Porte a peage, seul passage entre les poches.
    if local_x == 11 && local_z == 19 {
        return true;
    }
    ((8..=10).contains(&local_x) || (12..=14).contains(&local_x)) && (18..=20).contains(&local_z)
}

/// Sol du carre : tout bloque (opcode 50 = BLOCK_MAP_SQUARE) sauf les
/// poches et la porte. Ordre serveur : niveau, x local, z local.
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

/// Un seul loc : la porte a peage en (BASE+11, BASE+19), mur droit vers
/// l'ouest (info = shape 0, angle 0 : le moteur ne prend le clic que
/// depuis l'ouest, le sens de la traversee), coordonnee 11*64+19 = 723.
fn loc_bytes() -> Vec<u8> {
    vec![11, 0x82, 0xD4, 0x00, 0x00, 0x00]
}

/// Zip minimal a entrees stockees : le lecteur du pack ne prend que ca.
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
    let path = std::env::temp_dir().join(format!("lifescape-paid-only-{}.zip", std::process::id()));
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
        toll: true,
        gate_leaf: None,
    });
    let world = StaticWorld::open(&path, defs).expect("synthetic world");
    let tables = Tables {
        transitions: Vec::new(),
        verified: HashMap::new(),
        verified_any_level: Vec::new(),
        proven: HashMap::new(),
        requirements: Vec::new(),
        one_way: Vec::new(),
        customs: Vec::new(),
        blocked: Vec::new(),
        sky_floors: HashSet::new(),
        loc_names: Vec::new(),
    };
    (world, tables, path)
}

#[test]
fn toll_gate_alone_is_paid_only_then_reachable_with_paid() {
    let (world, tables, path) = setup();
    let unpaid = NavOptions::default();
    let start = std::time::Instant::now();
    let blocked = plan::plan(&world, &tables, from_tile(), dest_tile(), &unpaid);
    let unpaid_ms = start.elapsed().as_millis();
    let denied = blocked.expect_err("unpaid plan must not cross the toll gate");
    assert_eq!(denied.reason, NoRouteReason::PaidOnly, "the gate is the only passage");
    assert_eq!(denied.closest_reachable, dest_tile(), "coins, not walls, block the way");
    assert!(
        denied.blocked_by_requirements.iter().any(|(_, tags)| tags.iter().any(|tag| tag == "coins:10")),
        "the toll diagnosis names its price: {:?}",
        denied.blocked_by_requirements,
    );
    let mut paid = NavOptions::default();
    paid.allow_paid = true;
    let start = std::time::Instant::now();
    let crossed = plan::plan(&world, &tables, from_tile(), dest_tile(), &paid);
    let paid_ms = start.elapsed().as_millis();
    let crossed = crossed.expect("paid plan crosses the toll gate");
    assert!(
        crossed.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { door, .. } if door.loc == DOOR_ID)),
        "paid plan swings the toll gate",
    );
    println!("paid_only: PaidOnly [{unpaid_ms}ms] paid_ok [{paid_ms}ms]");
    let _ = std::fs::remove_file(&path);
}

// -- starvation golden (admin-7526 live) --------------------------------------
// A ~440k-tile pocket burns the shared wall budget in search A, so B/C/D
// inherit an expired deadline, stop at their first 1024-visit check and
// the toll diagnosis degrades into a bare Disconnected. Post-fix B gets
// a fresh budget and names the price. Timings are debug-calibrated
// (~45k visits/s here): A burns ~10 s (or its whole budget under load,
// a cut B's toll path still outranks), B needs ~16k visits.

const WIDE_SQUARES: i32 = 11;
const WALL_X: i32 = BASE + 630;
const GATE_Z: i32 = BASE + 352;

fn wide_start() -> Tile {
    Tile { x: WALL_X - 70, z: GATE_Z, level: DOOR_LEVEL }
}

fn wide_dest() -> Tile {
    Tile { x: WALL_X + 1, z: GATE_Z, level: DOOR_LEVEL }
}

/// Smart encoding (pack reader): one byte under 128, else two bytes
/// biased by 32768.
fn smart(value: i32, out: &mut Vec<u8>) {
    if value < 128 {
        out.push(value as u8);
    } else {
        let biased = (value + 32768) as u16;
        out.push((biased >> 8) as u8);
        out.push((biased & 0xFF) as u8);
    }
}

fn wide_setup() -> (std::sync::Arc<StaticWorld>, Tables, std::path::PathBuf) {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for square_x in SQUARE..SQUARE + WIDE_SQUARES {
        for square_z in SQUARE..SQUARE + WIDE_SQUARES {
            let mut ground = Vec::with_capacity(4 * 64 * 64);
            for level in 0..4 {
                for local_x in 0..64 {
                    for local_z in 0..64 {
                        let (x, z) = (square_x * 64 + local_x, square_z * 64 + local_z);
                        let wall = level == 0 && x == WALL_X && z != GATE_Z;
                        if level == 0 && !wall {
                            ground.push(0);
                        } else {
                            ground.push(50);
                            ground.push(0);
                        }
                    }
                }
            }
            let locs = if square_x == WALL_X / 64 && square_z == GATE_Z / 64 {
                let (local_x, local_z) = (WALL_X % 64, GATE_Z % 64);
                let mut bytes = Vec::new();
                smart(DOOR_ID as i32 + 1, &mut bytes);
                smart(local_x * 64 + local_z + 1, &mut bytes);
                bytes.push(0);
                smart(0, &mut bytes);
                smart(0, &mut bytes);
                bytes
            } else {
                vec![0]
            };
            files.push((format!("m{square_x}_{square_z}"), ground));
            files.push((format!("l{square_x}_{square_z}"), locs));
        }
    }
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (name.as_str(), data.as_slice())).collect();
    let zip = stored_zip(&refs);
    let path = std::env::temp_dir().join(format!("lifescape-paid-wide-{}.zip", std::process::id()));
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
        toll: true,
        gate_leaf: None,
    });
    let world = StaticWorld::open(&path, defs).expect("synthetic world");
    let tables = Tables {
        transitions: Vec::new(),
        verified: HashMap::new(),
        verified_any_level: Vec::new(),
        proven: HashMap::new(),
        requirements: Vec::new(),
        one_way: Vec::new(),
        customs: Vec::new(),
        blocked: Vec::new(),
        sky_floors: HashSet::new(),
        loc_names: Vec::new(),
    };
    (world, tables, path)
}

#[test]
fn starved_search_still_diagnoses_paid_only() {
    let (world, tables, path) = wide_setup();
    let unpaid = NavOptions::default();
    let denied = plan::plan(&world, &tables, wide_start(), wide_dest(), &unpaid)
        .expect_err("unpaid plan must not cross the toll gate");
    assert_eq!(denied.reason, NoRouteReason::PaidOnly, "starved A must not eat the toll diagnosis, got {:?}", denied.reason);
    assert_eq!(denied.closest_reachable, wide_dest(), "coins, not walls, block the way");
    assert!(
        denied.blocked_by_requirements.iter().any(|(_, tags)| tags.iter().any(|tag| tag == "coins:10")),
        "the toll diagnosis names its price: {:?}",
        denied.blocked_by_requirements,
    );
    let mut paid = NavOptions::default();
    paid.allow_paid = true;
    let crossed = plan::plan(&world, &tables, wide_start(), wide_dest(), &paid).expect("paid plan crosses the toll gate");
    assert!(
        crossed.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { door, .. } if door.loc == DOOR_ID)),
        "paid plan swings the toll gate",
    );
    let _ = std::fs::remove_file(&path);
}
