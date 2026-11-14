use super::*;

use std::sync::Arc;
use std::time::Instant;

use crate::service::Snapshot;
use crate::service_queries::{static_locs_near, toll_fence};
use crate::static_world::StaticWorld;

fn world() -> Arc<StaticWorld> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    StaticWorld::open(&zip, defs).expect("static world")
}

fn snapshot() -> Arc<Snapshot> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    Snapshot::new(world(), Arc::new(tables), "region".to_string())
}

/// The pre-2026-11-14 service scan, kept as the reference: one tile
/// read (two locks) per diamond tile, strict improvement.
fn reference_toll_fence(world: &StaticWorld, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Option<(i32, i32, i32)> {
    let radius = radius.max(0);
    let mut best: Option<((i32, i32, i32), i32)> = None;
    for scan_x in tile_x - radius..=tile_x + radius {
        let rest = radius - (scan_x - tile_x).abs();
        for scan_z in tile_z - rest..=tile_z + rest {
            if world.is_toll(scan_x, scan_z, level) {
                let dist = (scan_x - tile_x).abs() + (scan_z - tile_z).abs();
                if best.is_none_or(|(_, known)| dist < known) {
                    best = Some(((scan_x, scan_z, level), dist));
                }
            }
        }
    }
    best.map(|(tile, _)| tile)
}

fn reference_static_locs_near(world: &StaticWorld, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Vec<StaticLoc> {
    let radius = radius.max(0);
    let mut out = Vec::new();
    for scan_x in tile_x - radius..=tile_x + radius {
        let rest = radius - (scan_x - tile_x).abs();
        for scan_z in tile_z - rest..=tile_z + rest {
            if let Some((loc, shape, angle)) = world.loc_at(scan_x, scan_z, level) {
                out.push(StaticLoc { x: scan_x, z: scan_z, level, loc, shape, angle });
            }
        }
    }
    out.sort_by_key(|entry| ((entry.x - tile_x).abs() + (entry.z - tile_z).abs(), entry.x, entry.z, entry.loc));
    out
}

/// Region reads equal the per-tile scans (Al-Kharid fence, Lumbridge,
/// Draynor, a square border), and the micro-benchmark prints the gain:
/// fresh worlds each side so painting is paid once by both.
#[test]
fn region_reads_match_the_per_tile_scans() {
    let centers = [(3265, 3227, 0), (3268, 3227, 0), (3222, 3218, 0), (3091, 3242, 0), (3200, 3200, 0), (3268, 3200, 1)];
    let old_world = world();
    let fresh = snapshot();
    for (x, z, level) in centers {
        for radius in [0, 1, 10, 64, 150] {
            assert_eq!(
                toll_fence(&fresh, x, z, level, radius),
                reference_toll_fence(&old_world, x, z, level, radius),
                "toll_fence ({x},{z},{level}) r{radius}"
            );
        }
        for radius in [0, 2, 20, 64] {
            assert_eq!(
                static_locs_near(&fresh, x, z, level, radius),
                reference_static_locs_near(&old_world, x, z, level, radius),
                "static_locs_near ({x},{z},{level}) r{radius}"
            );
        }
    }
    // Micro-benchmark (warm worlds, same queries): per-call time.
    let rounds = 20;
    let started = Instant::now();
    for round in 0..rounds {
        reference_toll_fence(&old_world, 3250 + round, 3227, 0, 150);
    }
    let old_toll = started.elapsed() / rounds as u32;
    let started = Instant::now();
    for round in 0..rounds {
        toll_fence(&fresh, 3250 + round, 3227, 0, 150);
    }
    let new_toll = started.elapsed() / rounds as u32;
    let started = Instant::now();
    for round in 0..rounds {
        reference_static_locs_near(&old_world, 3090 + round, 3242, 0, 64);
    }
    let old_locs = started.elapsed() / rounds as u32;
    let started = Instant::now();
    for round in 0..rounds {
        static_locs_near(&fresh, 3090 + round, 3242, 0, 64);
    }
    let new_locs = started.elapsed() / rounds as u32;
    eprintln!("BENCH toll_fence r150: per-tile {old_toll:?} -> region {new_toll:?}");
    eprintln!("BENCH static_locs_near r64: per-tile {old_locs:?} -> region {new_locs:?}");
}

/// A covering list answers every query inside its diamond exactly like
/// the service, refuses the queries that leave it, and never crosses
/// levels.
#[test]
fn covering_lists_answer_exactly_inside_and_refuse_outside() {
    let fresh = snapshot();
    let (center_x, center_z, level, radius, margin) = (3262, 3227, 0, 150, 48);
    let tolls = TollList {
        x: center_x,
        z: center_z,
        level,
        radius: radius + margin,
        gates: fresh.world.toll_gates_within(center_x, center_z, level, radius + margin),
    };
    assert!(!tolls.gates.is_empty(), "the Al-Kharid gate lies inside");
    let locs = LocsList {
        x: center_x,
        z: center_z,
        level,
        radius: 40,
        locs: static_locs_near(&fresh, center_x, center_z, level, 40),
    };
    for step_x in -48..=48 {
        for step_z in [-30, -7, 0, 3, 25, 48] {
            let (x, z) = (center_x + step_x, center_z + step_z);
            match tolls.answer(x, z, level, radius) {
                Some(answer) => assert_eq!(answer, toll_fence(&fresh, x, z, level, radius), "toll at ({x},{z})"),
                None => assert!(step_x.abs() + step_z.abs() > margin, "covered query refused at ({x},{z})"),
            }
            for small in [2, 10] {
                match locs.answer(x, z, level, small) {
                    Some(answer) => assert_eq!(answer, static_locs_near(&fresh, x, z, level, small), "locs at ({x},{z}) r{small}"),
                    None => assert!(step_x.abs() + step_z.abs() + small > 40, "covered locs refused at ({x},{z})"),
                }
            }
        }
    }
    assert_eq!(tolls.answer(center_x, center_z, 1, radius), None, "other level never answers");
    assert_eq!(tolls.answer(center_x + margin + 1, center_z, level, radius), None, "outside the diamond refuses");
}

#[test]
fn gates_round_trip_and_absent_reads_none() {
    let gates = vec![(3268, 3227, 0), (3268, 3228, 0)];
    let reply = lifescape_json::object(vec![(GATES_FIELD, encode_gates(&gates))]);
    assert_eq!(decode_gates(&reply), Some(gates));
    assert_eq!(decode_gates(&lifescape_json::object(vec![])), None, "old daemon: no list, no cache");
}
