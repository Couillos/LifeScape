//! Danger-aware routing: the Lumbridge -> Draynor walk passes the jail
//! guards (vislevel 26, hunt everyone under combat 53) at (3121,3249)
//! and (3127,3248); bots died there on every bank trip. With the danger
//! squares in the options, the plan detours around them when the map
//! allows it, and still finds a route when it does not.

use std::sync::Arc;

use lifescape_nav::plan::{self, DangerZone, NavOptions, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

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

fn walked(plan: &plan::Plan) -> Vec<Tile> {
    plan.segments
        .iter()
        .flat_map(|segment| match segment {
            Segment::Walk { tiles } => tiles.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn inside(zones: &[DangerZone], tile: Tile) -> bool {
    zones.iter().any(|zone| zone.level == tile.level && (tile.x - zone.x).abs().max((tile.z - zone.z).abs()) <= zone.radius)
}

#[test]
fn route_detours_around_danger_and_still_arrives() {
    let (world, tables) = setup();
    let from = Tile { x: 3222, z: 3218, level: 0 };
    let bank = Tile { x: 3092, z: 3243, level: 0 };
    // Jail guards: huntrange 5 + wanderrange 11 + 1 (scripts `kit::danger`).
    let guards = vec![
        DangerZone { x: 3121, z: 3249, level: 0, radius: 17 },
        DangerZone { x: 3127, z: 3248, level: 0, radius: 17 },
    ];
    let plain = NavOptions { allow_paid: false, f2p_only: true, ..NavOptions::default() };
    let wary = NavOptions { danger: guards.clone(), ..plain.clone() };
    let straight = plan::plan(&world, &tables, from, bank, &plain).expect("plain route");
    let careful = plan::plan(&world, &tables, from, bank, &wary).expect("a route still exists with danger");
    let exposed_before = walked(&straight).into_iter().filter(|tile| inside(&guards, *tile)).count();
    let exposed_after = walked(&careful).into_iter().filter(|tile| inside(&guards, *tile)).count();
    eprintln!("tiles in danger: {exposed_before} plain, {exposed_after} with danger; walk {} -> {}", walked(&straight).len(), walked(&careful).len());
    assert!(exposed_before > 0, "the plain road passes the guards (the live deaths), got 0");
    assert_eq!(exposed_after, 0, "a detour exists (live: 35 tiles -> 0 for 2 more steps), got {exposed_after}");
    // The destination itself inside a square still plans (cost, never a ban).
    let in_zone = Tile { x: 3121, z: 3262, level: 0 };
    assert!(plan::plan(&world, &tables, from, in_zone, &wary).is_ok(), "a dangerous destination still plans");
}
