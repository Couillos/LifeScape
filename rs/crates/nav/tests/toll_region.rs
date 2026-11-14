//! Test d'or de la région du péage d'Al Kharid (2026-11-14) : la région
//! « derrière le péage » se dérive de la connexité de la carte servie
//! (`toll_side::derive_region` : remplissage de marche, portes ordinaires
//! ouvertes, porte à péage fermée), jamais d'une droite x = 3268. Live
//! bd23dqpq : le banc de pêche de la côte sud (3267,3148) comptait pour
//! le côté gratuit (x < 3268), lancé sans `toll`, NoRoute, pêche bannie.
//!
//! Le fichier `gamedata/data/toll_region.json` doit égaler la dérivation ;
//! `LIFESCAPE_BLESS_TOLL_REGION=1` le réécrit après un changement de carte.

use std::sync::Arc;

use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;
use lifescape_nav::toll_side::{derive_region, flood, region_json, TollRegion, Window};

/// Porte à péage (`border_gate.rs2`, loc 2882) : sa case est le côté Al Kharid.
const GATE: (i32, i32, i32) = (3268, 3227, 0);
/// Fenêtre de dérivation : Rimmington, Port Sarim et Draynor à l'ouest (côté
/// gratuit au sud de la clôture), le désert à l'est, la
/// côte au sud ; `max_z` est le plafond de la recherche vers le nord.
const BASE: Window = Window { min_x: 2880, min_z: 3100, max_x: 3400, max_z: 3450 };

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// Données de ce dépôt (jamais celles d'un autre checkout).
fn gamedata_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gamedata/data")
}

fn setup() -> (Arc<StaticWorld>, Tables) {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(gamedata_dir().join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(gamedata_dir().join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let tables = Tables::load(&gamedata_dir()).expect("tables");
    (world, tables)
}

fn inside(region: &TollRegion, tile_x: i32, tile_z: i32) -> bool {
    region.rows.iter().any(|(row_z, spans)| *row_z == tile_z && spans.iter().any(|(start, end)| (*start..=*end).contains(&tile_x)))
}

#[test]
fn toll_region_matches_the_served_map() {
    let (world, tables) = setup();
    let region = derive_region(&world, &tables, GATE, BASE).expect("the gate separates its sides");
    // La clôture s'arrête au nord à (3270,3329) : au-dessus, les deux
    // côtés se rejoignent à pied (le détour gratuit par le nord).
    assert_eq!(region.window.max_z, 3329, "fence top");
    let window = region.window;
    let west = flood(&world, &tables, (GATE.0 - 1, GATE.1), 0, window);
    let east = flood(&world, &tables, (GATE.0, GATE.1), 0, window);
    assert_eq!(east.intersection(&west).count(), 0, "the gate splits the window");
    // Côté Al Kharid : la côte sud à l'ouest de x = 3268 (le banc de
    // bd23dqpq et sa rive), la banque, la mine, le palais.
    for (tile_x, tile_z, what) in [
        (3267, 3148, "south coast fishing spot (water)"),
        (3267, 3149, "its shore"),
        (3269, 3167, "Al Kharid bank"),
        (3298, 3300, "Al Kharid mine"),
        (3293, 3170, "palace"),
        (3268, 3227, "gate tile, east side"),
    ] {
        assert!(inside(&region, tile_x, tile_z), "{what} ({tile_x},{tile_z}) is behind the toll");
    }
    // Côté gratuit : Lumbridge, la case ouest de la porte, les vaches à
    // l'est de Lumbridge, le haut de la clôture (elle passe à x 3270 au nord).
    for (tile_x, tile_z, what) in [
        (3222, 3218, "Lumbridge castle"),
        (3267, 3227, "gate tile, west side"),
        (3255, 3275, "Lumbridge east cows"),
        (3268, 3329, "west of the fence top (the fence bends to x 3270 up north)"),
        (3290, 3340, "north of the fence top"),
    ] {
        assert!(!inside(&region, tile_x, tile_z), "{what} ({tile_x},{tile_z}) is toll-free");
    }
    let expected = region_json(&region, GATE);
    let path = gamedata_dir().join("toll_region.json");
    if std::env::var("LIFESCAPE_BLESS_TOLL_REGION").is_ok() {
        std::fs::write(&path, &expected).expect("bless toll_region.json");
    }
    let stored = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(stored == expected, "gamedata/data/toll_region.json differs from the served map: rerun with LIFESCAPE_BLESS_TOLL_REGION=1");
}
