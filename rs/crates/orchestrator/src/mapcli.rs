//! Offline map diagnostics: no daemon, no session, no login.
//!
//! Reads the server pack straight off disk (shared page cache, nothing
//! resident) and answers the questions `debug_walk` cannot:
//! `map route` plans BFS between two tiles, `map flags` dumps one tile,
//! `map corridor` renders walkability ASCII, `map booth` finds the nearest
//! bank booth. `BFS_TRACE=1` traces every queued tile.

use std::collections::HashMap;

use lifescape_sdk::collision::CollisionMap;

use crate::staticmap;

fn engine_dir() -> String {
    std::env::var("LIFESCAPE_ENGINE_DIR").unwrap_or_else(|_| {
        let root = crate::paths::root();
        root.join("../lostcity/engine").to_string_lossy().to_string()
    })
}

fn parse_triplet(args: &[String], at: usize) -> Option<(i32, i32, i32)> {
    let x: i32 = args.get(at)?.parse().ok()?;
    let z: i32 = args.get(at + 1)?.parse().ok()?;
    let level: i32 = args.get(at + 2).and_then(|raw| raw.parse().ok()).unwrap_or(0);
    Some((x, z, level))
}

pub fn map_cli(args: &[String]) -> i32 {
    let usage = "usage: orchestrator map route <x1> <z1> <x2> <z2> [level]\n       orchestrator map flags <x> <z> [level]\n       orchestrator map corridor <x1> <z1> <x2> <z2> [level]\n       orchestrator map booth <x> <z> [radius]\n       orchestrator map locs <x> <z> [radius]";
    let sub = args.first().map(String::as_str).unwrap_or("");
    let mut pack = match staticmap::build_pack(&engine_dir()) {
        Ok(pack) => pack,
        Err(error) => {
            eprintln!("map pack missing: {error}");
            return 1;
        }
    };
    let mut collision = CollisionMap::new();
    let mut locs: HashMap<(i32, i32, i32), (u16, u8, u8)> = HashMap::new();
    match sub {
        "route" => {
            let num = |index: usize| args.get(index).and_then(|raw| raw.parse::<i32>().ok());
            let (Some(src_x), Some(src_z), Some(dest_x), Some(dest_z)) =
                (num(1), num(2), num(3), num(4)) else {
                eprintln!("{usage}");
                return 2;
            };
            let level: i32 = args.get(5).and_then(|raw| raw.parse().ok()).unwrap_or(0);
            let mut doors = std::collections::HashMap::new();
            let mut toll_gates = std::collections::HashSet::new();
            pack.ensure_area(&mut collision, &mut locs, &mut doors, &mut toll_gates, src_x, src_z, 70);
            pack.ensure_area(&mut collision, &mut locs, &mut doors, &mut toll_gates, dest_x, dest_z, 10);
            let collision = pack.planning_view(&collision, &doors, &toll_gates, false);
            let mut router = lifescape_sdk::route::Router::new();
            let waypoints = router.find_path_to_tile(&collision, level, src_x, src_z, dest_x, dest_z);
            if waypoints.is_empty() {
                let report = router.unreachable_report(
                    &collision, level, src_x, src_z, dest_x, dest_z, None, 8, 1, 1, 0,
                    lifescape_sdk::reach::SHAPE_TILE, 0,
                );
                println!("no route ({src_x},{src_z}) -> ({dest_x},{dest_z},{level}): nearest={:?} blocked={} blockers={:?}",
                    report.nearest, report.dest_blocked, report.blockers);
                println!("neighbors of src:");
                for (name, dx, dz, mask) in [("W", -1, 0, 0x240108u32), ("E", 1, 0, 0x240180u32), ("S", 0, -1, 0x240102u32), ("N", 0, 1, 0x240120u32)] {
                    let flags = collision.get(src_x + dx, src_z + dz, level);
                    println!("  {name} ({},{}) flags={flags:#x} enterable={}", src_x + dx, src_z + dz, (flags & mask) == 0);
                }
                println!("BFS visited map (+=src, X=dest, #=reached, .=unreached):");
                let mut visited: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
                {
                    let mut probe = lifescape_sdk::route::Router::new();
                    probe.find_path_to_tile(&collision, level, src_x, src_z, dest_x, dest_z);
                    for (index, dist) in probe.distances().iter().enumerate() {
                        if *dist < 99999999 {
                            let index = index as i32;
                            visited.insert((src_x - 64 + index / 128, src_z - 64 + index % 128));
                        }
                    }
                }
                let (lo_x, hi_x) = (src_x.min(dest_x) - 4, src_x.max(dest_x) + 4);
                let (lo_z, hi_z) = (src_z.min(dest_z) - 4, src_z.max(dest_z) + 4);
                for z in (lo_z..=hi_z).rev() {
                    let mut row = format!("{z:4} ");
                    for x in lo_x..=hi_x {
                        let c = if x == src_x && z == src_z { '+' }
                        else if x == dest_x && z == dest_z { 'X' }
                        else if visited.contains(&(x, z)) { '#' }
                        else if lifescape_sdk::collision::tile_walkable(collision.get(x, z, level)) { '.' }
                        else { ' ' };
                        row.push(c);
                    }
                    println!("{row}");
                }
                return 1;
            }
            println!("{} waypoints ({},{}) -> ({},{}):", waypoints.len(), src_x, src_z, dest_x, dest_z);
            for waypoint in &waypoints {
                println!("  ({},{},{})", waypoint.x, waypoint.z, waypoint.level);
            }
            0
        }
        "flags" => {
            let Some((x, z, level)) = parse_triplet(args, 1) else {
                eprintln!("{usage}");
                return 2;
            };
            let mut doors = std::collections::HashMap::new();
            let mut toll_gates = std::collections::HashSet::new();
            pack.ensure_area(&mut collision, &mut locs, &mut doors, &mut toll_gates, x, z, 1);
            let flags = collision.get(x, z, level);
            println!("({x},{z},{level}) flags={flags:#x} walkable={} loaded={}",
                lifescape_sdk::collision::tile_walkable(flags),
                if collision.has(x, z, level) { "yes" } else { "no (void)" });
            0
        }
        "corridor" => {
            let (Some((x1, z1, _)), Some((x2, z2, level))) =
                (parse_triplet(args, 1), parse_triplet(args, 3)) else {
                eprintln!("{usage}");
                return 2;
            };
            let (lo_x, hi_x) = (x1.min(x2), x1.max(x2));
            let (lo_z, hi_z) = (z1.min(z2), z1.max(z2));
            let mut doors = std::collections::HashMap::new();
            let mut toll_gates = std::collections::HashSet::new();
            pack.ensure_area(&mut collision, &mut locs, &mut doors, &mut toll_gates, (lo_x + hi_x) / 2, (lo_z + hi_z) / 2,
                ((hi_x - lo_x).max(hi_z - lo_z) / 2 + 2).max(70));
            for z in (lo_z..=hi_z).rev() {
                let mut row = format!("{z:4} ");
                for x in lo_x..=hi_x {
                    let blocked_here = (x, z) == (x1, z1) || (x, z) == (x2, z2);
                    let c = if blocked_here {
                        'X'
                    } else if lifescape_sdk::collision::tile_walkable(collision.get(x, z, level)) {
                        '.'
                    } else {
                        '#'
                    };
                    row.push(c);
                }
                println!("{row}");
            }
            0
        }
        "booth" => {
            let Some((x, z, _)) = parse_triplet(args, 1) else {
                eprintln!("{usage}");
                return 2;
            };
            let radius: i32 = args.get(3).and_then(|raw| raw.parse().ok()).unwrap_or(40);
            let mut doors = std::collections::HashMap::new();
            let mut toll_gates = std::collections::HashSet::new();
            pack.ensure_area(&mut collision, &mut locs, &mut doors, &mut toll_gates, x, z, radius);
            let mut best: Option<(i64, (i32, i32, u16))> = None;
            for ((tile_x, tile_z, tile_level), (loc, _, _)) in &locs {
                if *tile_level != 0 {
                    continue;
                }
                let name = lifescape_gamedata::loc_by_id(*loc as usize)
                    .and_then(|entry| entry.name)
                    .unwrap_or_default()
                    .to_lowercase();
                if !name.contains("bankbooth") && !name.contains("bank booth") {
                    continue;
                }
                let dist = i64::from((tile_x - x).abs() + (tile_z - z).abs());
                if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                    best = Some((dist, (*tile_x, *tile_z, *loc)));
                }
            }
            match best {
                Some((dist, (booth_x, booth_z, booth_loc))) => {
                    println!("booth id={booth_loc} at ({booth_x},{booth_z}) manhattan={dist}");
                    0
                }
                None => {
                    println!("no bankbooth within {radius} of ({x},{z}): static={} live=0", locs.len());
                    1
                }
            }
        }
        "locs" => {
            let Some((x, z, _)) = parse_triplet(args, 1) else {
                eprintln!("{usage}");
                return 2;
            };
            let radius: i32 = args.get(4).and_then(|raw| raw.parse().ok()).unwrap_or(10);
            let mut doors = std::collections::HashMap::new();
            let mut toll_gates = std::collections::HashSet::new();
            pack.ensure_area(&mut collision, &mut locs, &mut doors, &mut toll_gates, x, z, radius);
            let mut rows: Vec<_> = locs.iter().filter(|((lx, lz, level), _)| {
                *level == 0 && (lx - x).abs() <= radius && (lz - z).abs() <= radius
            }).collect();
            rows.sort_by_key(|((lx, lz, _), _)| (lx - x).abs() + (lz - z).abs());
            for ((lx, lz, level), (id, shape, angle)) in rows.into_iter().take(40) {
                let name = lifescape_gamedata::loc_by_id(*id as usize)
                    .and_then(|entry| entry.name)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("#{id}"));
                println!("{name} id={id} at ({lx},{lz},{level}) shape={shape} angle={angle}");
            }
            0
        }
        _ => {
            eprintln!("{usage}");
            2
        }
    }
}
