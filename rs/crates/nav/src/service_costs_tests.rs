//! `walk_costs` batches (service side): one search each, priced per tile.

use std::collections::HashSet;
use std::sync::Arc;

use crate::plan::{DangerZone, NavOptions, Tile};
use crate::service::{test_socket, PlanClient, Service, Snapshot};

fn service(prefix: &str) -> Arc<Service> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = crate::static_world::StaticWorld::open(&zip, defs).expect("world");
    let zones = crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    Service::start(Snapshot::new(world, Arc::new(tables), "costs-set".to_string()), &test_socket(prefix), 1)
        .expect("service starts")
}

fn options(danger: Vec<DangerZone>) -> NavOptions {
    NavOptions { allow_paid: false, f2p_only: true, satisfied: HashSet::new(), excluded_edges: HashSet::new(), danger }
}

fn costs_hits(client: &PlanClient) -> i64 {
    let reply = client.request(&lifescape_json::object(vec![("op", lifescape_json::str_value("stats"))])).expect("stats");
    reply.get("walk_costs_hits").as_i64().expect("walk_costs_hits counter")
}

/// A re-ordered or repeated window prices the same per tile (costs
/// follow their tiles), whatever the order the batch lists them in; a
/// danger set and a moved start price their own search. There is no batch
/// cache any more (lot N6: 1 % live hit rate, the key holds the exact
/// start tile): every batch is a search, none counts as a cache hit.
#[test]
fn walk_costs_price_each_tile_whatever_the_window_order() {
    let service = service("costs-set");
    let client = PlanClient::new(service.socket_path(), "costs-set");
    let from = Tile { x: 3230, z: 3241, level: 0 };
    let near = Tile { x: 3203, z: 3246, level: 0 };
    let far = Tile { x: 3228, z: 3214, level: 0 };
    let (first, _) = client.walk_costs(from, &[far, near], &options(Vec::new()), "costs-set").expect("serves");
    let (swapped, _) = client.walk_costs(from, &[near, far, near], &options(Vec::new()), "costs-set").expect("serves");
    assert_eq!(swapped, vec![first[1], first[0], first[1]], "costs follow their tiles");
    let (again, _) = client.walk_costs(from, &[far, near], &options(Vec::new()), "costs-set").expect("serves");
    assert_eq!(again, first, "the same batch prices the same");
    assert_eq!(costs_hits(&client), 0, "no batch cache: nothing is served without a search");
    let danger = vec![DangerZone { x: 3215, z: 3243, level: 0, radius: 3 }];
    let (priced, _) = client.walk_costs(from, &[far, near], &options(danger), "costs-set").expect("serves");
    assert_eq!(priced.len(), 2);
    let moved = Tile { x: 3231, z: 3241, level: 0 };
    let (from_moved, _) = client.walk_costs(moved, &[far, near], &options(Vec::new()), "costs-set").expect("serves");
    assert_eq!(from_moved.len(), 2);
    assert_eq!(service.stats().walk_costs, 5, "five batches, five searches");
}
