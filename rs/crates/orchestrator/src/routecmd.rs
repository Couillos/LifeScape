//! Offline route planner: no daemon, no session, no login.
//!
//! Builds the static world straight off the server pack, loads the
//! transition dump plus prober verdicts and door rules, runs nav::plan
//! and prints the segments. `--ascii` draws every walk leg.

use std::sync::Arc;

use lifescape_nav::plan::{self, NavOptions, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

use crate::paths;

pub fn engine_dir() -> String {
    std::env::var("LIFESCAPE_ENGINE_DIR").unwrap_or_else(|_| {
        let root = crate::paths::root();
        root.join("../lostcity/engine").to_string_lossy().to_string()
    })
}

fn ascii_walk(tiles: &[Tile]) {
    if tiles.is_empty() {
        return;
    }
    if tiles.len() > 120 {
        println!("    (walk too long for ascii: {} tiles)", tiles.len());
        return;
    }
    let (mut lo_x, mut hi_x, mut lo_z, mut hi_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for tile in tiles {
        lo_x = lo_x.min(tile.x);
        hi_x = hi_x.max(tile.x);
        lo_z = lo_z.min(tile.z);
        hi_z = hi_z.max(tile.z);
    }
    lo_x -= 2;
    hi_x += 2;
    lo_z -= 2;
    hi_z += 2;
    let path: std::collections::HashSet<(i32, i32)> = tiles.iter().map(|tile| (tile.x, tile.z)).collect();
    let (start, end) = ((tiles[0].x, tiles[0].z), (tiles[tiles.len() - 1].x, tiles[tiles.len() - 1].z));
    for z in (lo_z..=hi_z).rev() {
        let mut row = format!("    {z:4} ");
        for x in lo_x..=hi_x {
            row.push(if (x, z) == start {
                'S'
            } else if (x, z) == end {
                'E'
            } else if path.contains(&(x, z)) {
                '*'
            } else {
                '.'
            });
        }
        println!("{row}");
    }
}

/// Static world plus planner tables, no daemon, no session, no login.
pub fn load_offline() -> Result<(Arc<StaticWorld>, Tables), String> {
    let root = paths::root();
    let data_dir = root.join("gamedata").join("data");
    let engine = engine_dir();
    let zip = std::path::Path::new(&engine).join("data").join("pack").join(".cache").join("maps-server.zip");
    let defs = match crate::staticmap::load_loctypes(&data_dir.join("gametypes.json")) {
        Ok(defs) => defs,
        Err(err) => return Err(err),
    };
    let world = StaticWorld::open(&zip, defs).map_err(|err| err.to_string())?;
    let content = std::path::Path::new(&engine).join("../content");
    match lifescape_nav::f2p::load_zones(&content.join("maps").join("free2play.csv")) {
        Ok(zones) => world.set_f2p(zones),
        Err(err) => eprintln!("route: f2p bounds missing ({err}), everything reads inside"),
    }
    let tables = Tables::load(&data_dir).map_err(|err| err)?;
    world.set_sky_floors(tables.sky_floors.clone());
    Ok((world, tables))
}

pub fn route_cmd(args: &[String]) -> i32 {
    let usage = "usage: orchestrator route <x1> <z1> <niv1> <x2> <z2> <niv2> [--paid] [--ascii]";
    let (mut paid, mut ascii) = (false, false);
    let mut nums = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--paid" => paid = true,
            "--ascii" => ascii = true,
            _ => match arg.parse::<i32>() {
                Ok(num) => nums.push(num),
                Err(_) => {
                    eprintln!("{usage}");
                    return 2;
                }
            },
        }
    }
    if nums.len() != 6 {
        eprintln!("{usage}");
        return 2;
    }
    let (from, dest) = (
        Tile { x: nums[0], z: nums[1], level: nums[2] },
        Tile { x: nums[3], z: nums[4], level: nums[5] },
    );
    let (world, tables) = match load_offline() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("route: {err}");
            return 1;
        }
    };
    let mut options = NavOptions::default();
    options.allow_paid = paid;
    match plan::plan(&world, &tables, from, dest, &options) {
        Ok(plan) => {
            println!(
                "route ({},{},{}) -> ({},{},{}){}: cost {} in {} segments",
                from.x,
                from.z,
                from.level,
                dest.x,
                dest.z,
                dest.level,
                if paid { " [paid]" } else { "" },
                plan.cost,
                plan.segments.len(),
            );
            for (index, seg) in plan.segments.iter().enumerate() {
                let tag = format!("seg {}/{}", index + 1, plan.segments.len());
                match seg {
                    Segment::Walk { tiles } => {
                        let (first, last) = (&tiles[0], &tiles[tiles.len() - 1]);
                        println!(
                            "{tag} Walk {} tiles ({},{},{})->({},{},{})",
                            tiles.len(),
                            first.x,
                            first.z,
                            first.level,
                            last.x,
                            last.z,
                            last.level,
                        );
                        if ascii {
                            ascii_walk(tiles);
                        }
                    }
                    Segment::OpenDoor { door, stand, cross_to } => {
                        println!(
                            "{tag} OpenDoor loc={} stand=({},{}) cross_to=({},{}) level={}",
                            door.loc, stand.x, stand.z, cross_to.x, cross_to.z, door.level,
                        );
                    }
                    Segment::Ride { transition, stand, pre_open, expect } => {
                        let mut line = format!(
                            "{tag} Ride {} loc={} climb_op={} stand=({},{}) expect=({},{},{})",
                            transition.name,
                            transition.climb_loc,
                            transition.op,
                            stand.x,
                            stand.z,
                            expect.x,
                            expect.z,
                            expect.level,
                        );
                        if let Some(open) = pre_open {
                            line += &format!(" pre_open loc={} op={}", open.loc, open.op).to_string();
                        }
                        println!("{line}");
                    }
                    Segment::Custom { edge, stand, expect } => {
                        println!(
                            "{tag} Custom {} stand=({},{}) expect=({},{},{})",
                            edge.name, stand.x, stand.z, expect.x, expect.z, expect.level,
                        );
                    }
                }
            }
            if let Some(adjusted) = plan.adjusted_dest {
                println!("adjusted_dest ({},{},{})", adjusted.x, adjusted.z, adjusted.level);
            }
            0
        }
        Err(no) => {
            println!(
                "no route ({},{},{}) -> ({},{},{}): {} closest=({},{},{})",
                from.x,
                from.z,
                from.level,
                dest.x,
                dest.z,
                dest.level,
                no.reason.as_str(),
                no.closest_reachable.x,
                no.closest_reachable.z,
                no.closest_reachable.level,
            );
            for (edge, tags) in &no.blocked_by_requirements {
                println!("  blocked by {edge:?} missing {tags:?}");
            }
            if !no.disabled_transitions_nearby.is_empty() {
                println!("  disabled transitions nearby: {}", no.disabled_transitions_nearby.len());
            }
            1
        }
    }
}
