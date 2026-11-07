//! `plan_yielding`: `false` stoppe une longue recherche (Aborted), `true`
//! rend le meme plan que `plan()`.

use lifescape_nav::plan::{self, NavOptions, NoRouteReason, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn setup() -> (std::sync::Arc<StaticWorld>, Tables) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
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

#[test]
fn yielding_false_aborts_long_search() {
    let (world, tables) = setup();
    let from = Tile { x: 3222, z: 3222, level: 0 };
    let dest = Tile { x: 2500, z: 2500, level: 0 };
    let mut calls = 0;
    let failed =
        plan::plan_yielding(&world, &tables, from, dest, &NavOptions::default(), &mut || {
            calls += 1;
            false
        });
    let denied = failed.expect_err("a refused yield must abort, not plan");
    assert_eq!(denied.reason, NoRouteReason::Aborted, "abort maps to Aborted");
    assert!(calls >= 1, "the search must hit a yield point, got {calls}");
}

#[test]
fn yielding_true_matches_plan() {
    let (world, tables) = setup();
    let from = Tile { x: 3228, z: 3214, level: 0 };
    let dest = Tile { x: 3224, z: 3214, level: 0 };
    let options = NavOptions::default();
    let plain = plan::plan(&world, &tables, from, dest, &options).expect("short route plans");
    let yielded = plan::plan_yielding(&world, &tables, from, dest, &options, &mut || true).expect("yield-true plans too");
    assert_eq!(format!("{plain:?}"), format!("{yielded:?}"), "yield-true is byte-identical to plan()");
}
