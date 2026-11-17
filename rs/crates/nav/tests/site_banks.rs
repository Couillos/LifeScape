//! The bank serving each site, recomputed from the map (tour 3 point 2).
//!
//! `gamedata/data/site_banks.json` prices every site of
//! `gamedata::site_banks::catalogue()` against the F2P banks with the
//! planner's own costs (`lifescape_nav::site_banks::serving_banks`) and
//! keeps the cheapest route: the offline dumper. On mismatch the test
//! fails; `LIFESCAPE_WRITE_SITE_BANKS=1` writes the table.

use std::sync::Arc;

use lifescape_nav::plan::Tile;
use lifescape_nav::site_banks::serving_banks;
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

#[test]
fn site_banks_match_the_map() {
    let (world, tables) = setup();
    let banks = lifescape_gamedata::bank_stands::bank_entries();
    assert_eq!(banks.len(), 7, "the seven F2P banks of rev 274");
    let stands: Vec<Tile> = banks.iter().map(|bank| Tile { x: bank.stand.0 as i32, z: bank.stand.1 as i32, level: bank.booth.2 as i32 }).collect();
    let sites = lifescape_gamedata::site_banks::catalogue();
    assert!(sites.len() > 1000, "catalogue read: {}", sites.len());
    let tiles: Vec<Tile> = sites.iter().map(|(x, z, level)| Tile { x: *x as i32, z: *z as i32, level: *level as i32 }).collect();
    let started = std::time::Instant::now();
    let served = serving_banks(&world, &tables, &tiles, &stands, lifescape_gamedata::site_banks::PRICED_RADIUS as i32).expect("edges");
    eprintln!("site_banks: {} sites priced in {:?}", sites.len(), started.elapsed());
    let bank_rows: Vec<String> = banks
        .iter()
        .map(|bank| format!("{{\"name\":\"{}\",\"booth\":[{},{},{}],\"stand\":[{},{}]}}", bank.name, bank.booth.0, bank.booth.1, bank.booth.2, bank.stand.0, bank.stand.1))
        .collect();
    let site_rows: Vec<String> = sites
        .iter()
        .zip(&served)
        .map(|((x, z, level), served)| match served {
            Some(served) => format!("[{x},{z},{level},{},{}]", served.bank, served.cost),
            None => format!("[{x},{z},{level}]"),
        })
        .collect();
    // The live N3 site: Draynor (index of `draynor`), never Al Kharid.
    let swamp = sites.iter().enumerate().filter(|(_, site)| site.2 == 0).min_by_key(|(_, site)| (site.0 - 3244).abs().max((site.1 - 3165).abs())).map(|(index, _)| served[index]);
    let draynor = banks.iter().position(|bank| bank.name == "draynor");
    assert!(swamp.flatten().map(|served| served.bank) == draynor, "the swamp mine banks at Draynor: {swamp:?}");
    let table = format!(
        "{{\"_doc\":\"Bank serving each site by route cost (rs/crates/nav/tests/site_banks.rs): banks = the F2P banks (first booth, its first client stand); sites = [x,z,level,bank index,route cost], or [x,z,level] when no bank route within {} tiles.\",\n\"banks\":[\n{}\n],\n\"sites\":[\n{}\n]}}\n",
        lifescape_gamedata::site_banks::PRICED_RADIUS,
        bank_rows.join(",\n"),
        site_rows.join(",\n")
    );
    let path = data_dir().join("site_banks.json");
    if std::env::var("LIFESCAPE_WRITE_SITE_BANKS").is_ok() {
        std::fs::write(&path, &table).expect("write site_banks.json");
    }
    let shipped = std::fs::read_to_string(&path).unwrap_or_default();
    let served_count = served.iter().filter(|served| served.is_some()).count();
    assert!(shipped == table, "site_banks.json drifted from the map; rerun with LIFESCAPE_WRITE_SITE_BANKS=1 ({served_count} of {} sites served)", sites.len());
}
