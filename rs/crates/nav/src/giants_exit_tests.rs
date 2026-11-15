//! Hill giants' hall exit under live danger (`detour.rs`) and the guild
//! pocket it must keep proving: needs the engine pack like the other
//! world tests.
//!
//! Live 2026-11-15 (alicefield since 03:00, kevin_ellis since 08:46):
//! every route out of the hall (3109,9841) answered NeedsRequirement on
//! the brass-key door 1804. Workers send their danger squares with every
//! plan (`kit::danger::arm`, combat 33: the giants and the dungeon
//! hunters). The lifted path leaves by the cellar ladder into the key
//! house after a few danger steps (453 to Lumbridge), the strict detour
//! walks ~170 danger tiles through the gate and the main dungeon (1980):
//! past the old `2 x lifted + 64` cap (970), so no detour was "found".
//! The dossier carries no danger, hence the offline replay disagreed.

use std::sync::{Arc, OnceLock};

use crate::plan::{self, NavOptions, NoRouteReason, Segment, SharedEdges, Tile};
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

/// Live options of a combat-33 bot standing in the hall: F2P, no key,
/// the danger squares its worker arms.
fn live_options(from: Tile) -> NavOptions {
    NavOptions { f2p_only: true, danger: crate::service_replay_live::live_danger(from, 33), ..NavOptions::default() }
}

fn crosses_key_door(found: &plan::Plan) -> bool {
    found.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { door, .. } if door.loc == 1804))
}

#[test]
fn giants_hall_exits_under_live_danger() {
    let fixture = fixture();
    let from = Tile { x: 3109, z: 9841, level: 0 };
    let options = live_options(from);
    assert!(!options.danger.is_empty(), "the hall is armed for combat 33");
    for (dest_x, dest_z) in [(3236, 3210), (3097, 3510), (3194, 3396)] {
        let dest = Tile { x: dest_x, z: dest_z, level: 0 };
        let outcome = plan::plan_memo(&fixture.world, &fixture.tables, from, dest, &options, &fixture.shared, &fixture.memo, &mut || true);
        match outcome {
            Ok(found) => {
                assert!(!crosses_key_door(&found), "{dest:?}: no key, never door 1804");
                assert_eq!(found.adjusted_dest, None, "{dest:?}: walkable target reached");
            }
            Err(no) => panic!("{dest:?}: the trapdoor detour exists, got {:?} blocked {:?}", no.reason, no.blocked_by_requirements),
        }
    }
}

/// A requirement door guarding a true pocket still answers
/// NeedsRequirement (champions' guild, qp:32, no detour): the escalation
/// proves the pocket instead of sweeping the world.
#[test]
fn guild_pocket_still_needs_requirement() {
    let fixture = fixture();
    let from = Tile { x: 3191, z: 3366, level: 0 };
    let dest = Tile { x: 3191, z: 3361, level: 0 };
    let started = std::time::Instant::now();
    let outcome = plan::plan_memo(&fixture.world, &fixture.tables, from, dest, &NavOptions::default(), &fixture.shared, &fixture.memo, &mut || true);
    match outcome {
        Err(no) => assert_eq!(no.reason, NoRouteReason::NeedsRequirement),
        Ok(found) => panic!("guild entered without qp: {found:?}"),
    }
    assert!(started.elapsed() < std::time::Duration::from_millis(1500), "pocket proof stays fast: {:?}", started.elapsed());
}
