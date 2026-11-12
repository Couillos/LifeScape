//! Search budget versus verdict: a search cut by its budget proved
//! nothing, so it must never read as `Disconnected` (nor as a plan
//! adjusted to wherever the cut left it).
//!
//! Live failure (george_97-19062, fiona_park-15499): the daemon's debug
//! pool cut the Falador/Draynor -> Varrock fountain legs at the wall
//! budget, the diagnosis searches ran out of their shared budget at the
//! first 1024-visit check, and the answer read `Disconnected
//! closest=(3039,3350,0)` / `(3103,3268,0)` — the exact tiles a forced
//! zero budget yields below. Offline the same legs plan (cost 313/298).

use std::time::Duration;

use crate::collision::tile_walkable;
use crate::plan::{plan_budgeted, prepare_shared, NavOptions, NoRouteReason, Tile, DIAGNOSIS_BUDGET, PLAN_BUDGET};
use crate::service::SERVICE_WAIT_MAX;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

fn real_world() -> (std::sync::Arc<StaticWorld>, Tables) {
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
    (world, tables)
}

/// Live market options: F2P bounds, no tolls (the dossiers' `options`).
fn market_options() -> NavOptions {
    NavOptions { allow_paid: false, f2p_only: true, ..NavOptions::default() }
}

const FOUNTAIN: Tile = Tile { x: 3216, z: 3426, level: 0 };
const FALADOR_BANK: Tile = Tile { x: 3015, z: 3355, level: 0 };
const DRAYNOR_BANK: Tile = Tile { x: 3092, z: 3245, level: 0 };

#[test]
fn budget_cut_is_budget_exhausted_never_disconnected() {
    let (world, tables) = real_world();
    let options = market_options();
    let shared = prepare_shared(&world, &tables, &options, &mut || true).expect("edges");
    for (from, live_closest) in [(FALADOR_BANK, Tile { x: 3039, z: 3350, level: 0 }), (DRAYNOR_BANK, Tile { x: 3103, z: 3268, level: 0 })] {
        let cut = plan_budgeted(&world, &tables, from, FOUNTAIN, &options, &shared, &mut || true, Duration::ZERO)
            .expect_err("a zero budget cannot reach the fountain");
        assert_eq!(cut.reason.as_str(), "BudgetExhausted", "{from:?}: a cut search proved nothing, got {:?}", cut.reason);
        assert_eq!(cut.closest_reachable, live_closest, "{from:?}: the live dossier's closest is the cut point");
    }
}

#[test]
fn budget_cut_on_a_blocked_dest_never_adjusts_to_the_cut_point() {
    let (world, tables) = real_world();
    let options = market_options();
    let shared = prepare_shared(&world, &tables, &options, &mut || true).expect("edges");
    // Draynor bank booth (fiona_park opened it there): blocked, so a
    // finished search would adjust to its closest stand.
    let booth = Tile { x: 3091, z: 3245, level: 0 };
    assert!(!tile_walkable(world.open_flag(booth.x, booth.z, booth.level, false)), "the booth tile blocks");
    match plan_budgeted(&world, &tables, FALADOR_BANK, booth, &options, &shared, &mut || true, Duration::ZERO) {
        Ok(plan) => panic!("cut search served a plan adjusted to {:?}", plan.adjusted_dest),
        Err(no) => assert_eq!(no.reason.as_str(), "BudgetExhausted", "got {:?}", no.reason),
    }
}

#[test]
fn long_f2p_market_legs_fit_the_budget() {
    let (world, tables) = real_world();
    let options = market_options();
    let shared = prepare_shared(&world, &tables, &options, &mut || true).expect("edges");
    for (from, cost) in [(FALADOR_BANK, 313), (DRAYNOR_BANK, 298)] {
        let plan = plan_budgeted(&world, &tables, from, FOUNTAIN, &options, &shared, &mut || true, PLAN_BUDGET)
            .unwrap_or_else(|no| panic!("{from:?} -> fountain: {:?} closest={:?}", no.reason, no.closest_reachable));
        assert_eq!(plan.cost, cost, "{from:?}: offline `route` cost");
    }
}

#[test]
fn budget_reason_round_trips_and_never_caches() {
    let reason = NoRouteReason::parse("BudgetExhausted").expect("known reason");
    assert_eq!(reason.as_str(), "BudgetExhausted");
    assert!(!reason.cacheable(), "a cut depends on load, the next submit must search again");
    assert!(!NoRouteReason::Aborted.cacheable());
    for proven in [NoRouteReason::Disconnected, NoRouteReason::NeedsRequirement, NoRouteReason::PaidOnly, NoRouteReason::OutsideF2p] {
        assert!(proven.cacheable(), "{proven:?} is a finished verdict");
        assert_eq!(NoRouteReason::parse(proven.as_str()), Ok(proven));
    }
    // A, then B/C/D on one shared budget: the worst plan must still
    // answer inside the client's wait, or the bot reads nav-down instead.
    assert!(PLAN_BUDGET + DIAGNOSIS_BUDGET < SERVICE_WAIT_MAX, "{PLAN_BUDGET:?} + {DIAGNOSIS_BUDGET:?} must stay under {SERVICE_WAIT_MAX:?}");
}
