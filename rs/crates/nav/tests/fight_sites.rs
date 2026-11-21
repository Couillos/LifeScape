//! Fight sites of the combat camps, recomputed from the map.
//!
//! `gamedata/data/unreachable_spawns.json` lists every F2P spawn of a
//! combat camp (`combat_spots::catalogue_spots`) no stand reaches in the
//! open world (`lifescape_nav::site_reach::spawn_verdicts`: bars, walls,
//! a sealed room). `combat_spots::positions` leaves them out, so the
//! brain's site table and the ladder's clusters never aim at one (live
//! 2026-11-21: the caged lesser demon at the top of the Wizards' Tower
//! (3110,3157,2), `Disconnected` on every walk, 25 failures in 6 h).
//!
//! An underground spawn (z >= 6400) must also be routed from the surface
//! both ways (`lifescape_nav::site_reach::route_verdicts`, seed the
//! Lumbridge spawn, F2P, fares carried): a hall behind the slashable web
//! or Elvarg's lair behind the one-way secret door is a large region the
//! pocket flood calls open, yet no plan enters it (`no_route`). Every
//! underground camp spawn left in the open world is therefore one the
//! navigation reaches (user 2026-11-21: the brain fights there,
//! `access::fight_gates_at`).
//! Recomputed here; on mismatch the message prints the table,
//! `LIFESCAPE_WRITE_SITES=1` writes it.

use std::sync::Arc;

use lifescape_nav::plan::{NavOptions, Tile};
use lifescape_nav::site_reach::{route_verdicts, spawn_verdicts, SiteVerdict};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// This checkout's own game data (a worktree reads and writes its own).
fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gamedata/data")
}

fn setup() -> (Arc<StaticWorld>, Tables) {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(data_dir().join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(data_dir().join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&data_dir()).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    world.set_npc_blockers(tables.npc_blockers.clone());
    (world, tables)
}

/// `(x, z, level, spot)` of every F2P spawn of every catalogue camp.
fn camp_spawns() -> Vec<(i64, i64, i64, &'static str)> {
    let mut out: Vec<(i64, i64, i64, &'static str)> = Vec::new();
    for spot in lifescape_gamedata::combat_spots::catalogue_spots() {
        for (x, z, level) in lifescape_gamedata::combat_spots::raw_positions(spot.key) {
            if !out.iter().any(|known| (known.0, known.1, known.2) == (x, z, level)) {
                out.push((x, z, level, spot.key));
            }
        }
    }
    out.sort();
    out
}

/// Where every route verdict starts: the Lumbridge spawn.
const SEED: Tile = Tile { x: 3222, z: 3218, level: 0 };

/// The live walker's view of a fighter carrying its fares (`orchestrator
/// route --paid`): F2P, paid edges open, the boat's coins held.
fn paid_options(tables: &Tables) -> NavOptions {
    let mut options = NavOptions { f2p_only: true, allow_paid: true, ..NavOptions::default() };
    options.satisfied.extend(tables.customs.iter().filter(|edge| edge.fare > 0).map(|edge| format!("coins:{}", edge.fare)));
    options.satisfied.extend(tables.customs.iter().filter(|edge| edge.fare > 0).flat_map(|edge| edge.requires.iter().filter(|tag| tag.starts_with("coins:")).cloned()));
    options
}

/// Verdict of every camp spawn (`SiteVerdict::reason`, or `no_route`):
/// the pocket rule, then the route rule for the underground ones the
/// pocket leaves open.
fn camp_verdicts(world: &StaticWorld, tables: &Tables, spawns: &[(i64, i64, i64, &'static str)]) -> Vec<&'static str> {
    let tiles: Vec<Tile> = spawns.iter().map(|(x, z, level, _)| Tile { x: *x as i32, z: *z as i32, level: *level as i32 }).collect();
    let verdicts = spawn_verdicts(world, tables, &tiles).expect("edges");
    let below: Vec<usize> = (0..tiles.len()).filter(|index| verdicts[*index] == SiteVerdict::Reachable && tiles[*index].z >= 6400).collect();
    let below_tiles: Vec<Tile> = below.iter().map(|index| tiles[*index]).collect();
    let routed = route_verdicts(world, tables, &paid_options(tables), SEED, &below_tiles).expect("edges");
    let mut out: Vec<&'static str> = verdicts.iter().map(SiteVerdict::reason).collect();
    for (index, routed) in below.iter().zip(routed) {
        if !routed {
            out[*index] = "no_route";
        }
    }
    out
}

#[test]
fn unreachable_fight_sites_match_the_map() {
    let (world, tables) = setup();
    let spawns = camp_spawns();
    assert!(spawns.len() > 200, "F2P camp spawns read: {}", spawns.len());
    let verdicts = camp_verdicts(&world, &tables, &spawns);
    let mut computed: Vec<String> = Vec::new();
    for ((x, z, level, spot), verdict) in spawns.iter().zip(&verdicts) {
        if *verdict != "reachable" && *verdict != "no_loc" {
            computed.push(format!("{{\"x\":{x},\"z\":{z},\"level\":{level},\"spot\":\"{spot}\",\"reason\":\"{verdict}\"}}"));
        }
    }
    let verdict_at = |at: (i64, i64, i64)| spawns.iter().zip(&verdicts).find(|((x, z, level, _), _)| (*x, *z, *level) == at).map(|(_, verdict)| *verdict);
    // The caged demon is out; the volcano's six stay (boat and rope);
    // the lair's five past the secret door are refused (Dragon Slayer).
    assert_eq!(verdict_at((3110, 3157, 2)), Some("sealed"), "the caged demon (3110,3157,2)");
    let volcano: Vec<_> =
        spawns.iter().zip(&verdicts).filter(|((x, z, level, spot), _)| *spot == "lesser_demons" && *level == 0 && (2820..=2850).contains(x) && (9548..=9570).contains(z)).collect();
    assert_eq!(volcano.len(), 6, "the volcano's six demons");
    for ((x, z, _, _), verdict) in volcano {
        assert_eq!(*verdict, "reachable", "volcano demon ({x},{z})");
    }
    for lair in [(2835, 9602, 0), (2838, 9610, 0), (2845, 9612, 0)] {
        assert_eq!(verdict_at(lair), Some("no_route"), "lair demon {lair:?}");
    }
    // The Edgeville dungeon giants (trapdoor, no key) and the Asgarnian
    // ice dungeon (Mudskipper Point ladder) are routed; the moss giants
    // behind the slashable web are not.
    assert_eq!(verdict_at((3110, 9845, 0)), Some("reachable"), "a hill giant");
    assert_eq!(verdict_at((3053, 9576, 0)), Some("reachable"), "an ice warrior");
    assert_eq!(verdict_at((3157, 9905, 0)), Some("no_route"), "a moss giant behind the web");
    let table = format!("{{\"sites\":[\n{}\n]}}\n", computed.join(",\n"));
    let path = data_dir().join("unreachable_spawns.json");
    if std::env::var("LIFESCAPE_WRITE_SITES").is_ok() {
        std::fs::write(&path, &table).expect("write unreachable_spawns.json");
    }
    let shipped = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(shipped, table, "unreachable_spawns.json drifted from the map; rerun with LIFESCAPE_WRITE_SITES=1 ({} of {} spawns unreachable)", computed.len(), spawns.len());
}
