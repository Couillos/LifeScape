//! Client stands of every F2P bank booth, recomputed from the map.
//!
//! `gamedata/data/bank_stands.json` lists, per F2P bank, a public tile
//! outside the building and, per booth, the stands the server accepts the
//! booth op from (`reach::reached`, side tile with no wall on the shared
//! edge) split in two: `client` (reachable from the outside tile) and
//! `teller` (the bankers' side of the counter, a sealed pocket). The SDK
//! `open_bank` only ever walks to a `client` stand: aiming a teller stand
//! cost one exhaustive NoRoute search per try (3-11 s under load, ~1000
//! failure dossiers a day at Varrock west/east and Falador west).
//!
//! Recomputed here, both ways cheap: a teller stand plans *out* to the
//! outside tile in milliseconds (its pocket exhausts at once,
//! `Disconnected`); a client stand is planned *in* from the outside tile
//! (`Found`, walk only, exact destination). Any other verdict fails loud.
//! On mismatch the message prints the recomputed table to paste back.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use lifescape_json::Value;
use lifescape_nav::collision::tile_walkable;
use lifescape_nav::no_route::NoRouteReason;
use lifescape_nav::plan::{self, NavOptions, Segment, Tile};
use lifescape_nav::reach;
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

/// Bank booth loc (`interface_bank/configs/bank_booth.loc`, `bankbooth`).
const BOOTH_LOC: u16 = 2213;
/// A booth belongs to the bank whose outside tile is this close.
const BANK_RADIUS: i32 = 24;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn setup() -> (Arc<StaticWorld>, Tables) {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    (world, tables)
}

fn read_json(name: &str) -> Value {
    let path = workspace_root().join("LifeScape/gamedata/data").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    lifescape_json::parse(&text).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn tile_of(value: &Value) -> Tile {
    Tile {
        x: value.get("x").as_i64().expect("x") as i32,
        z: value.get("z").as_i64().expect("z") as i32,
        level: value.get("level").as_i64().expect("level") as i32,
    }
}

fn pairs_of(value: &Value) -> Vec<(i32, i32)> {
    value
        .as_arr()
        .expect("stand list")
        .iter()
        .map(|pair| {
            let coords = pair.as_arr().expect("stand pair");
            (coords[0].as_i64().expect("stand x") as i32, coords[1].as_i64().expect("stand z") as i32)
        })
        .collect()
}

/// Every free-to-play bankbooth spawn of the game dump.
fn f2p_booths() -> Vec<Tile> {
    let locs = read_json("locs.json");
    let mut booths = Vec::new();
    for row in locs.as_arr().expect("locs rows") {
        if row.get("name").as_str() != Some("bankbooth") {
            continue;
        }
        for spawn in row.get("spawns").as_arr().expect("spawns") {
            if spawn.get("members").as_bool() == Some(true) {
                continue;
            }
            booths.push(tile_of(spawn));
        }
    }
    booths.sort_by_key(|tile| (tile.x, tile.z, tile.level));
    booths
}

/// Server reach stands of one booth on the static map (same rule as the
/// SDK `loc_stands::stand_tiles`: rotated footprint and its ring, each
/// standable and passing `reach::reached`).
fn reach_stands(world: &StaticWorld, booth: Tile) -> Vec<(i32, i32)> {
    let (loc, shape, angle) = world
        .locs_at(booth.x, booth.z, booth.level)
        .into_iter()
        .find(|(loc, _, _)| *loc == BOOTH_LOC)
        .unwrap_or_else(|| panic!("no bankbooth on the map at {booth:?}"));
    let def = world.loc_def(usize::from(loc)).expect("booth loc def");
    assert_eq!((def.width, def.length), (1, 1), "booth footprint at {booth:?}");
    let get_flag = |flag_x: i32, flag_z: i32, flag_level: i32| world.get(flag_x, flag_z, flag_level);
    let mut stands = Vec::new();
    for stand_x in booth.x - 1..=booth.x + 1 {
        for stand_z in booth.z - 1..=booth.z + 1 {
            if !tile_walkable(world.get(stand_x, stand_z, booth.level)) {
                continue;
            }
            let reached = reach::reached(
                &get_flag,
                booth.level,
                stand_x,
                stand_z,
                booth.x,
                booth.z,
                def.width,
                def.length,
                i32::from(angle),
                i32::from(shape),
                0,
            );
            if reached {
                stands.push((stand_x, stand_z));
            }
        }
    }
    stands
}

fn chebyshev(first: Tile, second: Tile) -> i32 {
    (first.x - second.x).abs().max((first.z - second.z).abs())
}

fn f2p_options() -> NavOptions {
    NavOptions { f2p_only: true, ..NavOptions::default() }
}

/// True when the stand is on the client side: a walk-only plan from the
/// outside tile lands exactly on it. False when the stand's pocket cannot
/// reach the outside tile (`Disconnected`, planned outward). Panics on
/// any other verdict: the table must never guess.
fn client_side(world: &StaticWorld, tables: &Tables, outside: Tile, stand: Tile) -> bool {
    let started = Instant::now();
    let outward = plan::plan(world, tables, stand, outside, &f2p_options());
    if let Err(no_route) = &outward {
        assert_eq!(
            no_route.reason,
            NoRouteReason::Disconnected,
            "stand {stand:?} -> outside {outside:?}: {}",
            no_route.reason.as_str()
        );
        let spent = started.elapsed();
        assert!(spent.as_millis() < 2000, "teller stand {stand:?} verdict took {spent:?}");
        return false;
    }
    let inward = plan::plan(world, tables, outside, stand, &f2p_options())
        .unwrap_or_else(|no_route| panic!("outside {outside:?} -> stand {stand:?}: {}", no_route.reason.as_str()));
    assert!(inward.adjusted_dest.is_none(), "client stand {stand:?} reached exactly, got {:?}", inward.adjusted_dest);
    assert!(
        inward.segments.iter().all(|segment| matches!(segment, Segment::Walk { .. } | Segment::OpenDoor { .. })),
        "client stand {stand:?} is a walk from outside {outside:?}"
    );
    true
}

fn pairs_text(pairs: &[(i32, i32)]) -> String {
    let inner: Vec<String> = pairs.iter().map(|(pair_x, pair_z)| format!("[{pair_x}, {pair_z}]")).collect();
    format!("[{}]", inner.join(", "))
}

#[test]
fn bank_stand_table_matches_the_map() {
    let (world, tables) = setup();
    let table = read_json("bank_stands.json");
    let banks = table.get("banks").as_arr().expect("banks");
    let booths = f2p_booths();
    assert_eq!(booths.len(), 32, "F2P bankbooth spawns in rev 274");
    // name -> (outside, booth -> (client, teller)), recomputed.
    let mut computed: BTreeMap<String, (Tile, BTreeMap<(i32, i32, i32), (Vec<(i32, i32)>, Vec<(i32, i32)>)>)> =
        BTreeMap::new();
    let mut listed: BTreeMap<String, BTreeMap<(i32, i32, i32), (Vec<(i32, i32)>, Vec<(i32, i32)>)>> = BTreeMap::new();
    for bank in banks {
        let name = bank.get("name").as_str().expect("bank name").to_string();
        let outside = tile_of(bank.get("outside"));
        computed.insert(name.clone(), (outside, BTreeMap::new()));
        let mut rows = BTreeMap::new();
        for booth in bank.get("booths").as_arr().expect("booths") {
            let at = tile_of(booth);
            rows.insert((at.x, at.z, at.level), (pairs_of(booth.get("client")), pairs_of(booth.get("teller"))));
        }
        listed.insert(name, rows);
    }
    for booth in &booths {
        let owner = computed
            .iter()
            .filter(|(_, (outside, _))| outside.level == booth.level && chebyshev(*outside, *booth) <= BANK_RADIUS)
            .map(|(name, (outside, _))| (chebyshev(*outside, *booth), name.clone()))
            .min()
            .map(|(_, name)| name)
            .unwrap_or_else(|| panic!("booth {booth:?} has no bank with an outside tile within {BANK_RADIUS}"));
        let outside = computed[&owner].0;
        let (mut client, mut teller) = (Vec::new(), Vec::new());
        for (stand_x, stand_z) in reach_stands(&world, *booth) {
            let stand = Tile { x: stand_x, z: stand_z, level: booth.level };
            if client_side(&world, &tables, outside, stand) {
                client.push((stand_x, stand_z));
            } else {
                teller.push((stand_x, stand_z));
            }
        }
        assert!(!client.is_empty(), "booth {booth:?} ({owner}) has no client stand");
        computed.get_mut(&owner).expect("owner").1.insert((booth.x, booth.z, booth.level), (client, teller));
    }
    let mut lines = Vec::new();
    for (name, (outside, rows)) in &computed {
        lines.push(format!(
            "    {{\"name\": \"{name}\", \"outside\": {{\"x\": {}, \"z\": {}, \"level\": {}}}, \"booths\": [",
            outside.x, outside.z, outside.level
        ));
        let entries: Vec<String> = rows
            .iter()
            .map(|((booth_x, booth_z, level), (client, teller))| {
                format!(
                    "      {{\"x\": {booth_x}, \"z\": {booth_z}, \"level\": {level}, \"client\": {}, \"teller\": {}}}",
                    pairs_text(client),
                    pairs_text(teller)
                )
            })
            .collect();
        lines.push(entries.join(",\n"));
        lines.push("    ]}".to_string());
    }
    let recomputed: BTreeMap<String, _> = computed.iter().map(|(name, (_, rows))| (name.clone(), rows.clone())).collect();
    assert_eq!(listed, recomputed, "bank_stands.json is stale; recomputed banks:\n{}", lines.join("\n"));
}
