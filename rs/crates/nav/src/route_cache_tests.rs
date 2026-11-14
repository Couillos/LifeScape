//! Route cache (CPU 2026-11-14): pricing, option dominance, sub-paths,
//! splices, negatives TTL, version reset, LRU halving. Unit shapes on
//! synthetic plans first, then the real world through a real `Service`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use lifescape_json::Value;

use crate::plan::{self, DangerZone, DoorRef, EdgeId, NavOptions, Plan, Segment, Tile};
use crate::route_cache::{compatible, PlanEntry, RouteCache};
use crate::route_path::{analyze, plan_path, slice, Arrival};
use crate::route_splice::splice_epsilon;
use crate::service::{test_socket, PlanClient, PlanKey, Poll, Service, Snapshot};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

fn real_world() -> (Arc<StaticWorld>, Arc<Tables>) {
    static WORLD: std::sync::OnceLock<(Arc<StaticWorld>, Arc<Tables>)> = std::sync::OnceLock::new();
    WORLD
        .get_or_init(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
            let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
            let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
            let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
            let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
            let world = StaticWorld::open(&zip, defs).expect("static world");
            let zones = crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
            world.set_f2p(zones);
            let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
            world.set_sky_floors(tables.sky_floors.clone());
            (world, Arc::new(tables))
        })
        .clone()
}

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

fn f2p() -> NavOptions {
    NavOptions { f2p_only: true, ..NavOptions::default() }
}

fn walk_plan(count: i32) -> Plan {
    Plan {
        segments: vec![Segment::Walk { tiles: (1..=count).map(|x| tile(x, 0)).collect() }],
        cost: count as u32,
        adjusted_dest: None,
        dest_blocker: None,
    }
}

fn entry_of(world: &StaticWorld, tables: &Tables, from: Tile, dest: Tile, plan: Plan, options: &NavOptions) -> PlanEntry {
    let info = analyze(world, tables, from, &plan, &options.danger);
    PlanEntry {
        from,
        dest,
        plan,
        f2p_only: options.f2p_only,
        allow_paid: options.allow_paid,
        excluded: options.excluded_edges.clone(),
        danger: options.danger.clone(),
        info,
    }
}

/// Synthetic coordinates (no pack square): open ground, no door, so the
/// re-pricing is 1 per walk step and the door edges count per Dijkstra
/// edge (a merged on-then-off door is two edges, 2 x DOOR_COST).
#[test]
fn pricing_is_exact_per_step_and_per_door_edge() {
    let (world, tables) = real_world();
    let info = analyze(&world, &tables, tile(0, 0), &walk_plan(5), &[]);
    assert_eq!(info.cumulative, Some(vec![0, 1, 2, 3, 4, 5]));
    assert_eq!(info.cost_between(2, 5), Some(3), "suffix priced per tile");
    assert_eq!(plan_path(tile(0, 0), &Plan { segments: slice(&walk_plan(5), 0, 3), cost: 3, adjusted_dest: None, dest_blocker: None }).0.last(), Some(&tile(3, 0)));
    let door = DoorRef { loc: 7, x: 2, z: 0, level: 0, open_op: Some(1), toll: false };
    let merged = Plan {
        segments: vec![
            Segment::Walk { tiles: vec![tile(1, 0)] },
            Segment::OpenDoor { door: door.clone(), stand: tile(1, 0), cross_to: tile(3, 0) },
            Segment::Walk { tiles: vec![tile(4, 0)] },
        ],
        cost: 1 + 2 * plan::DOOR_COST + 1,
        adjusted_dest: None,
        dest_blocker: None,
    };
    let info = analyze(&world, &tables, tile(0, 0), &merged, &[]);
    assert_eq!(info.cumulative, Some(vec![0, 1, 1 + 2 * plan::DOOR_COST, 2 + 2 * plan::DOOR_COST]), "merged door: two edges");
    assert_eq!(info.arrival[2], Arrival::Door);
    assert!(!info.split[2], "a door landing never splits (search state)");
    assert_eq!(info.edges.len(), 1);
    assert_eq!(info.edges[0].id, EdgeId::Door { loc: 7, x: 2, z: 0, level: 0 });
    // A plan whose stated cost disagrees with the re-pricing serves whole
    // only: sub-paths would be mispriced.
    let mut wrong = walk_plan(5);
    wrong.cost = 9;
    assert!(analyze(&world, &tables, tile(0, 0), &wrong, &[]).cumulative.is_none());
}

/// Option dominance, unit level: tags, paid, bans and danger squares.
#[test]
fn compatibility_follows_the_subgraph_rules() {
    let (world, tables) = real_world();
    let door = DoorRef { loc: 7, x: 2, z: 0, level: 0, open_op: Some(1), toll: true };
    let tolled = Plan {
        segments: vec![
            Segment::Walk { tiles: vec![tile(1, 0)] },
            Segment::OpenDoor { door, stand: tile(1, 0), cross_to: tile(3, 0) },
            Segment::Walk { tiles: vec![tile(4, 0), tile(5, 0)] },
        ],
        cost: 1 + 2 * plan::DOOR_COST + 2,
        adjusted_dest: None,
        dest_blocker: None,
    };
    let paid = NavOptions { allow_paid: true, ..f2p() };
    let entry = entry_of(&world, &tables, tile(0, 0), tile(5, 0), tolled, &paid);
    assert!(compatible(&entry, &paid, 0, 4), "same options");
    assert!(!compatible(&entry, &f2p(), 0, 4), "toll crossed: a no-paid request recomputes");
    assert!(compatible(&entry, &f2p(), 2, 4), "after the toll: the tail is paid-free");
    let mut banned = paid.clone();
    banned.excluded_edges.insert(EdgeId::Door { loc: 7, x: 2, z: 0, level: 0 });
    assert!(!compatible(&entry, &banned, 0, 4), "a banned edge on the path recomputes");
    let mut tagged = paid.clone();
    tagged.satisfied.insert("coins:30".to_string());
    assert!(compatible(&entry, &tagged, 0, 4), "extra tags never change a found plan");
    let on_path = NavOptions { danger: vec![DangerZone { x: 4, z: 0, level: 0, radius: 0 }], ..paid.clone() };
    assert!(!compatible(&entry, &on_path, 0, 4), "a new danger square on a walked tile recomputes");
    let off_path = NavOptions { danger: vec![DangerZone { x: 40, z: 40, level: 0, radius: 2 }], ..paid.clone() };
    assert!(compatible(&entry, &off_path, 0, 4), "danger elsewhere only made other paths dearer");
    // An entry planned WITH danger never serves a request without it.
    let dangerous = entry_of(&world, &tables, tile(0, 0), tile(5, 0), walk_plan(5), &NavOptions { danger: off_path.danger.clone(), ..f2p() });
    assert!(!compatible(&dangerous, &f2p(), 0, 5), "fewer danger squares: other paths got cheaper");
}

/// Negatives expire, versions reset, inserts of another version drop,
/// the LRU halves oldest-first.
#[test]
fn negatives_versions_and_halving() {
    let (world, tables) = real_world();
    let options = f2p();
    let mut cache = RouteCache::new("v1");
    let key = PlanKey::of(tile(0, 0), tile(9, 9), &options, "v1");
    cache.insert_negative("v1", key.clone(), Value::Bool(false), Duration::ZERO);
    assert!(cache.fast("v1", &key, tile(0, 0), tile(9, 9), &options).is_none(), "expired negative recomputes");
    cache.insert_negative("v1", key.clone(), Value::Bool(false), Duration::from_secs(60));
    assert!(cache.fast("v1", &key, tile(0, 0), tile(9, 9), &options).is_some(), "live negative serves");
    for start in 0..4 {
        let entry = entry_of(&world, &tables, tile(start, 0), tile(start + 5, 0), walk_plan(5), &options);
        cache.insert_plan("v1", PlanEntry { from: tile(start, 0), dest: tile(start + 5, 0), ..entry });
    }
    assert_eq!(cache.snapshot_stats().entries, 4);
    cache.evict_half();
    assert_eq!(cache.snapshot_stats().entries, 2, "oldest half out");
    assert!(cache.fast("v1", &PlanKey::of(tile(3, 0), tile(8, 0), &options, "v1"), tile(3, 0), tile(8, 0), &options).is_some(), "newest kept");
    let stale = entry_of(&world, &tables, tile(0, 0), tile(5, 0), walk_plan(5), &options);
    cache.insert_plan("v0", stale);
    assert_eq!(cache.snapshot_stats().entries, 2, "a draining search of another version never lands");
    cache.reset("v2");
    assert_eq!(cache.len(), 0, "version bump empties the cache");
    assert!(cache.fast("v1", &key, tile(0, 0), tile(9, 9), &options).is_none(), "old version unreachable");
}

fn plan_once(client: &PlanClient, from: Tile, dest: Tile, options: &NavOptions, version: &str) -> Plan {
    for _ in 0..5 {
        if let Ok((Poll::Ready(plan), _)) = client.plan_once(from, dest, options, version, 20_000) {
            return plan;
        }
    }
    panic!("{from:?} -> {dest:?} never planned");
}

fn fresh(from: Tile, dest: Tile, options: &NavOptions) -> Plan {
    let (world, tables) = real_world();
    plan::plan(&world, &tables, from, dest, options).expect("fresh plan")
}

/// Every walk step of a served plan is one the planner could take.
fn assert_walkable(world: &StaticWorld, from: Tile, plan: &Plan, dest: Tile) {
    let mut at = from;
    for segment in &plan.segments {
        match segment {
            Segment::Walk { tiles } => {
                for next in tiles {
                    let level = at.level;
                    let mut flags = |x: i32, z: i32| world.get(x, z, level);
                    assert!(crate::bfs::can_step(&mut flags, at.x, at.z, next.x, next.z), "illegal step {at:?} -> {next:?}");
                    at = *next;
                }
            }
            Segment::OpenDoor { cross_to, .. } => at = *cross_to,
            Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => at = *expect,
        }
    }
    assert_eq!(at, dest, "served plan lands on the destination");
}

/// Real world, real service: a long leg once, then every reuse family
/// with no search — dominated options, prefix, suffix, splice — each
/// priced like a fresh plan (sub-paths exactly, splices within epsilon).
#[test]
fn service_reuses_one_leg_for_every_family() {
    let (world, tables) = real_world();
    let version = "route-cache";
    let service = Service::start(Snapshot::new(world.clone(), tables, version.to_string()), &test_socket("route-cache"), 2).expect("service");
    let client = PlanClient::new(service.socket_path(), "routes");
    let from = tile(3222, 3218);
    // A walkable destination (an adjusted one only reuses whole: its
    // closest tile depends on the start).
    let paid = NavOptions { allow_paid: true, ..f2p() };
    let dest = plan_once(&client, from, tile(3213, 3428), &paid, version).adjusted_dest.unwrap_or(tile(3213, 3428));
    let full = plan_once(&client, from, dest, &paid, version);
    assert!(full.adjusted_dest.is_none());
    let (path, arrival) = plan_path(from, &full);
    assert!(path.len() > 100, "a long leg");
    let before = service.stats();
    // Tags alone never change a found plan: an exact hit.
    let mut tagged = paid.clone();
    tagged.satisfied.insert("coins:30".to_string());
    assert_eq!(plan_once(&client, from, dest, &tagged, version).cost, full.cost);
    // Dominated: a paid-allowed leg that crosses no toll serves a no-paid bot.
    let served = plan_once(&client, from, dest, &f2p(), version);
    assert_eq!(served.cost, full.cost);
    assert_eq!(served.cost, fresh(from, dest, &f2p()).cost, "dominated plan priced like a fresh plan");
    let walk_at = |wanted: usize| (wanted..path.len()).find(|index| arrival[*index] == Arrival::Walk && world.door_at(path[*index].x, path[*index].z, 0).is_none()).expect("walk tile");
    // Prefix and suffix on split points: exact substructure.
    let mid = path[walk_at(path.len() / 2)];
    let prefix = plan_once(&client, from, mid, &f2p(), version);
    assert_eq!(prefix.cost, fresh(from, mid, &f2p()).cost, "prefix priced like a fresh plan");
    let suffix = plan_once(&client, mid, dest, &f2p(), version);
    assert_eq!(suffix.cost, fresh(mid, dest, &f2p()).cost, "suffix priced like a fresh plan");
    // Splice: a start two tiles off the path joins it by a short walk.
    let anchor = path[walk_at(20)];
    let off = [(2, 0), (-2, 0), (0, 2), (0, -2), (2, 2), (-2, -2), (2, -2), (-2, 2)]
        .iter()
        .map(|(step_x, step_z)| tile(anchor.x + step_x, anchor.z + step_z))
        .find(|candidate| {
            !path.contains(candidate)
                && world.door_at(candidate.x, candidate.z, 0).is_none()
                && plan::plan(&world, &real_world().1, *candidate, anchor, &f2p()).is_ok_and(|walk| walk.cost <= 3)
        })
        .expect("a tile two steps beside the path");
    let spliced = plan_once(&client, off, dest, &f2p(), version);
    let after = service.stats();
    assert_eq!(after.misses - before.misses, 0, "no search for any reuse");
    assert_eq!(after.exact_hits + after.dominated_hits - before.exact_hits - before.dominated_hits, 2, "whole-leg reuses");
    assert_eq!(after.trunc_hits - before.trunc_hits, 1);
    assert!(after.suffix_hits - before.suffix_hits >= 1);
    assert!(after.splice_hits - before.splice_hits + after.suffix_hits - before.suffix_hits >= 2, "the off-path start reuses the leg");
    assert_walkable(&world, off, &spliced, dest);
    let reference = fresh(off, dest, &f2p()).cost;
    assert!(spliced.cost >= reference && spliced.cost - reference <= splice_epsilon(reference), "splice {} vs fresh {reference}", spliced.cost);
    // A banned door on the leg is never served from it.
    if let Some(Segment::OpenDoor { door, .. }) = full.segments.iter().find(|segment| matches!(segment, Segment::OpenDoor { .. })) {
        let mut banned = f2p();
        banned.excluded_edges = HashSet::from([EdgeId::Door { loc: door.loc, x: door.x, z: door.z, level: door.level }]);
        let replanned = plan_once(&client, from, dest, &banned, version);
        assert!(!replanned.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { door: crossed, .. } if crossed.x == door.x && crossed.z == door.z)), "ban honoured");
    }
}
