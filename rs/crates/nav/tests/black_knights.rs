//! Forteresse des Chevaliers noirs (live 2026-11-17, ~60 dossiers/h sur 9
//! comptes qui cuisaient au feu du banquet (3030,3514)) : le plan entrait
//! par les grandes portes 73/74 (jamais ouvertes) ou par la porte gardee
//! 2337 depuis le sud (uniforme exige), et sortait par 73/74. Figes ici :
//! la sortie passe 2337 depuis son stand nord, sans diagonale, et
//! l'interieur (rez-de-chaussee et etage, relies par echelles internes)
//! est une poche scellee : `Disconnected` tout de suite, sans balayer le
//! monde F2P.

use std::sync::Arc;
use std::time::Instant;

use lifescape_nav::plan::{self, NavOptions, NoRouteReason, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir).join("../LifeScape");
        }
    }
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

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

fn f2p() -> NavOptions {
    NavOptions { f2p_only: true, ..NavOptions::default() }
}

/// Dedans (corridor ouest, salle du banquet) : la sortie passe 2337 depuis
/// (3016,3515) vers (3016,3514), un pas orthogonal a travers le mur nord
/// (le serveur teleporte le joueur sur la tuile de la porte), jamais les
/// grandes portes 73/74.
#[test]
fn inside_walks_out_through_the_front_door() {
    let (world, tables) = setup();
    for from in [tile(3016, 3515), tile(3029, 3515), tile(3010, 3515)] {
        let found = plan::plan(&world, &tables, from, tile(3094, 3491), &f2p())
            .unwrap_or_else(|no| panic!("{from:?}: the front door lets out, got {:?}", no.reason));
        let doors: Vec<(u16, Tile, Tile)> = found
            .segments
            .iter()
            .filter_map(|seg| match seg {
                Segment::OpenDoor { door, stand, cross_to } => Some((door.loc, *stand, *cross_to)),
                _ => None,
            })
            .collect();
        assert!(doors.iter().all(|(loc, _, _)| *loc != 73 && *loc != 74), "{from:?}: never the sealed gate, got {doors:?}");
        let front = doors.iter().find(|(loc, _, _)| *loc == 2337).unwrap_or_else(|| panic!("{from:?}: exits by 2337, got {doors:?}"));
        assert_eq!((front.1, front.2), (tile(3016, 3515), tile(3016, 3514)), "{from:?}: north stand, onto the door tile");
    }
}

/// Dehors : le corridor, le banquet et son feu ne s'atteignent pas, et le
/// verdict tombe vite (poche scellee, etage compris).
#[test]
fn outside_never_plans_in() {
    let (world, tables) = setup();
    for (from, dest) in [
        (tile(3031, 3516), tile(3029, 3515)),
        (tile(3094, 3489), tile(3030, 3513)),
        (tile(3094, 3489), tile(3016, 3515)),
        (tile(3017, 3514), tile(3016, 3515)),
        (tile(3006, 3516), tile(3008, 3515)),
    ] {
        let started = Instant::now();
        let result = plan::plan(&world, &tables, from, dest, &f2p());
        let elapsed = started.elapsed().as_millis();
        let no = result.err().unwrap_or_else(|| panic!("{from:?} -> {dest:?}: the fortress is closed from outside"));
        assert_eq!(no.reason, NoRouteReason::Disconnected, "{from:?} -> {dest:?}");
        if !cfg!(debug_assertions) {
            assert!(elapsed < 300, "{from:?} -> {dest:?}: sealed pocket answers fast, took {elapsed} ms");
        }
    }
}
