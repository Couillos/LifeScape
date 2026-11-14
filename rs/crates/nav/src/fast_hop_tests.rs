//! Short-hop fast path vs the full planner (`fast_hop.rs`): goldens and
//! a randomized equivalence over live-like requests. Needs the engine
//! pack like the other world tests.

use std::sync::{Arc, OnceLock};

use crate::fast_hop::{short_hop, Decline};
use crate::plan::{self, DangerZone, NavOptions, Plan, Segment, SharedEdges, Tile};
use crate::pocket::PocketMemo;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

struct Fixture {
    world: Arc<StaticWorld>,
    tables: Tables,
    shared: SharedEdges,
    memo: PocketMemo,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
        let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = StaticWorld::open(&zip, defs).expect("static world");
        world.set_f2p(crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p"));
        let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        world.set_sky_floors(tables.sky_floors.clone());
        let shared = plan::prepare_shared(&world, &tables, &NavOptions::default(), &mut || true).expect("edges");
        Fixture { world, tables, shared, memo: PocketMemo::default() }
    })
}

fn f2p() -> NavOptions {
    NavOptions { f2p_only: true, ..NavOptions::default() }
}

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

fn full(from: Tile, dest: Tile, options: &NavOptions) -> Result<Plan, plan::NoRoute> {
    let fixture = fixture();
    plan::plan_memo(&fixture.world, &fixture.tables, from, dest, options, &fixture.shared, &fixture.memo, &mut || true)
}

fn fast(from: Tile, dest: Tile, options: &NavOptions) -> Result<Plan, Decline> {
    let fixture = fixture();
    short_hop(&fixture.world, &fixture.shared, from, dest, options)
}

fn walked(plan: &Plan) -> Vec<Tile> {
    plan.segments
        .iter()
        .flat_map(|segment| match segment {
            Segment::Walk { tiles } => tiles.clone(),
            _ => Vec::new(),
        })
        .collect()
}

/// Asserts the fast answer equals the planner's: Ready both, same cost,
/// walk-only, exact destination. Returns whether the tiles match too.
fn assert_same(from: Tile, dest: Tile, options: &NavOptions, fast_plan: &Plan) -> bool {
    let reference = full(from, dest, options).unwrap_or_else(|no| panic!("{from:?}->{dest:?}: fast found cost {}, planner {:?}", fast_plan.cost, no.reason));
    assert_eq!(fast_plan.cost, reference.cost, "{from:?}->{dest:?} cost");
    assert_eq!(reference.adjusted_dest, None, "{from:?}->{dest:?} planner adjusted");
    assert!(reference.segments.iter().all(|segment| matches!(segment, Segment::Walk { .. })), "{from:?}->{dest:?}: planner used a door/ride at equal cost");
    assert_eq!(fast_plan.segments.len(), reference.segments.len(), "{from:?}->{dest:?} segment count");
    walked(fast_plan) == walked(&reference)
}

/// Goldens from the live logs (worker `nav plan` lines of 2026-11-14):
/// chicken farm re-align, Lumbridge, zero distance; costs as logged.
#[test]
fn golden_short_hops_match_planner_and_logs() {
    let cases = [
        (tile(3230, 3298), tile(3230, 3297), 1),
        (tile(3230, 3299), tile(3232, 3298), 2),
        (tile(3232, 3296), tile(3235, 3293), 3),
        (tile(3229, 3297), tile(3226, 3297), 3),
        (tile(3164, 3220), tile(3164, 3220), 0),
    ];
    for (from, dest, logged) in cases {
        let found = fast(from, dest, &f2p()).unwrap_or_else(|why| panic!("{from:?}->{dest:?} declined: {why:?}"));
        assert_eq!(found.cost, logged, "{from:?}->{dest:?}");
        assert_same(from, dest, &f2p(), &found);
    }
    let zero = fast(tile(3164, 3220), tile(3164, 3220), &f2p()).expect("zero hop");
    assert!(zero.segments.is_empty() && zero.adjusted_dest.is_none(), "zero hop: no segment, like the planner");
}

/// Blocked destination (a tree) is an adjusted plan: never answered
/// here. A hop through a door is the planner's (door edge, open op).
#[test]
fn golden_declines_blocked_dest_and_doors() {
    // Lumbridge castle courtyard tree south of the castle.
    let tree = (tile(3229, 3212), tile(3230, 3212));
    let outcome = fast(tree.0, tree.1, &f2p());
    if !crate::collision::tile_walkable(fixture().world.get(tree.1.x, tree.1.z, 0)) {
        assert_eq!(outcome.unwrap_err(), Decline::BlockedDest);
    }
    // Every Lumbridge hop the planner routes through a door: declined.
    let world = &fixture().world;
    let mut door_hops = 0;
    for door_x in 3200..3240 {
        for door_z in 3200..3260 {
            if world.door_at(door_x, door_z, 0).is_none() {
                continue;
            }
            for (from, dest) in [(tile(door_x - 2, door_z), tile(door_x + 2, door_z)), (tile(door_x, door_z - 2), tile(door_x, door_z + 2))] {
                let Ok(reference) = full(from, dest, &f2p()) else { continue };
                if reference.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { .. })) {
                    door_hops += 1;
                    let through = fast(from, dest, &f2p());
                    assert!(through.is_err(), "door hop {from:?}->{dest:?} answered: {through:?}");
                }
            }
        }
    }
    assert!(door_hops > 0, "Lumbridge has door hops");
    // Lumbridge castle door 1516: the planner clicks it (cost 23).
    let (castle_from, castle_dest) = (tile(3219, 3218), tile(3215, 3218));
    let castle = fast(castle_from, castle_dest, &f2p());
    println!("castle door hop: {castle:?} planner {:?}", full(castle_from, castle_dest, &f2p()).map(|plan| plan.cost));
    assert!(castle.is_err(), "castle door hop answered: {castle:?}");
    // Long and cross-level requests are never tried.
    assert_eq!(fast(tile(3222, 3218), tile(3240, 3218), &f2p()).unwrap_err(), Decline::NotShort);
    assert_eq!(fast(tile(3222, 3218), Tile { x: 3222, z: 3218, level: 1 }, &f2p()).unwrap_err(), Decline::NotShort);
}

/// Danger squares price the steps exactly like the planner (detour or
/// cross): a square on the straight line.
#[test]
fn golden_danger_detour_matches_planner() {
    let options = NavOptions { danger: vec![DangerZone { x: 3233, z: 3295, level: 0, radius: 0 }], ..f2p() };
    let (from, dest) = (tile(3232, 3296), tile(3235, 3293));
    match fast(from, dest, &options) {
        Ok(found) => {
            assert_same(from, dest, &options, &found);
            assert!(found.cost > 3, "the square prices the hop");
        }
        Err(why) => panic!("declined: {why:?}"),
    }
}

/// Deterministic LCG (no rand crate).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: i32) -> i32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % bound as u64) as i32
    }
}

/// Randomized equivalence: short requests around the live hot spots
/// (towns, farms, mines, forests), with no danger, the live danger list
/// of the start (`kit::danger::arm` rule) or a square on the hop. Every
/// answered hop must be the planner's verdict and cost.
#[test]
fn randomized_short_hops_equal_planner() {
    let centres = [
        (3231, 3297), (3222, 3218), (3208, 3248), (3094, 3243), (3185, 3436), (3253, 3420), (2965, 3380),
        (3289, 3365), (3180, 3370), (3105, 3433), (3165, 3300), (3274, 3189), (3036, 3254), (2990, 3195),
    ];
    let mut random = Lcg(0x5eed);
    let (mut answered, mut same_tiles, mut declined) = (0u32, 0u32, [0u32; 4]);
    for sample in 0..2400 {
        let (centre_x, centre_z) = centres[sample % centres.len()];
        let from = tile(centre_x + random.next(61) - 30, centre_z + random.next(61) - 30);
        if !crate::collision::tile_walkable(fixture().world.get(from.x, from.z, 0)) {
            continue;
        }
        let reach = [1, 1, 2, 3, 5, 8][random.next(6) as usize];
        let dest = tile(from.x + random.next(2 * reach + 1) - reach, from.z + random.next(2 * reach + 1) - reach);
        let mut options = f2p();
        match random.next(3) {
            0 => {}
            1 => options.danger = crate::service_replay_live::live_danger(from, 3),
            _ => options.danger = vec![DangerZone { x: (from.x + dest.x) / 2, z: (from.z + dest.z) / 2, level: 0, radius: random.next(3) }],
        }
        match fast(from, dest, &options) {
            Ok(found) => {
                answered += 1;
                if assert_same(from, dest, &options, &found) {
                    same_tiles += 1;
                }
            }
            Err(why) => declined[why as usize] += 1,
        }
    }
    println!("fast hops answered={answered} same_tiles={same_tiles} declined(not_short, blocked, unproven, not_found)={declined:?}");
    assert!(answered > 1000, "the fast path answers most short hops ({answered})");
}

/// Same equivalence on the REAL short requests of a replay extract
/// (ignored: `NAV_REPLAY=<extract>`, see `service_replay_tests`), with
/// the live danger list of each request.
#[test]
#[ignore]
fn replay_short_hops_equal_planner() {
    let Ok(path) = std::env::var("NAV_REPLAY") else { return };
    let text = std::fs::read_to_string(&path).expect("replay extract");
    let (mut answered, mut same_tiles, mut declined, mut logged_diff) = (0u32, 0u32, [0u32; 4], 0u32);
    for request in text.lines().filter_map(crate::service_replay_tests::parse_line) {
        let options = crate::service_replay_tests::request_options(&request);
        match fast(request.from, request.dest, &options) {
            Ok(found) => {
                answered += 1;
                if assert_same(request.from, request.dest, &options, &found) {
                    same_tiles += 1;
                }
                if found.cost != request.cost {
                    logged_diff += 1;
                }
            }
            Err(why) => declined[why as usize] += 1,
        }
    }
    println!(
        "replay fast hops answered={answered} same_tiles={same_tiles} declined(not_short, blocked, unproven, not_found)={declined:?} cost_differs_from_log={logged_diff}"
    );
}

/// `walk_costs` goal bound (`costs.rs`, CPU 2026-11-14): random batches
/// of walkable targets (rings around a random spot, some behind walls,
/// one far target now and then) still price every target exactly like
/// its unit plan when that plan costs <= the horizon, `None` past it.
#[test]
fn walk_costs_goal_bound_keeps_unit_prices() {
    let fixture = fixture();
    let mut random = Lcg(0xc0575);
    let (mut priced, mut beyond) = (0u32, 0u32);
    for sample in 0..120 {
        let (centre_x, centre_z) = [(3231, 3297), (3222, 3218), (3094, 3243), (3253, 3420), (2965, 3380), (3180, 3370)][sample % 6];
        let from = tile(centre_x + random.next(41) - 20, centre_z + random.next(41) - 20);
        let spot = tile(from.x + random.next(25) - 12, from.z + random.next(25) - 12);
        let mut dests: Vec<Tile> = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1)]
            .iter()
            .map(|(step_x, step_z)| tile(spot.x + step_x, spot.z + step_z))
            .filter(|dest| crate::collision::tile_walkable(fixture.world.get(dest.x, dest.z, 0)))
            .collect();
        if sample % 5 == 0 {
            dests.push(tile(from.x + 40, from.z - 30));
        }
        dests.retain(|dest| crate::collision::tile_walkable(fixture.world.get(dest.x, dest.z, 0)));
        if dests.is_empty() {
            continue;
        }
        let options = if sample % 2 == 0 { f2p() } else { NavOptions { danger: crate::service_replay_live::live_danger(from, 3), ..f2p() } };
        let costs = crate::costs::plan_costs_with(&fixture.world, &fixture.tables, from, &dests, &options, &fixture.shared, Some(&fixture.memo));
        let horizon = crate::costs::horizon(from, &dests);
        for (dest, cost) in dests.iter().zip(&costs) {
            let Ok(unit) = full(from, *dest, &options) else { continue };
            if unit.adjusted_dest.is_some() {
                continue;
            }
            if unit.cost <= horizon {
                assert_eq!(*cost, Some(unit.cost), "{from:?}->{dest:?} priced like its unit plan");
                priced += 1;
            } else {
                assert_eq!(*cost, None, "{from:?}->{dest:?} past the horizon {horizon}");
                beyond += 1;
            }
        }
    }
    println!("walk_costs goal bound: priced={priced} beyond_horizon={beyond}");
    assert!(priced > 200, "enough targets priced ({priced})");
}
